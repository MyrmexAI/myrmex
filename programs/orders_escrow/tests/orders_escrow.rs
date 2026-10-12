//! Сценарии эскроу: оплата с со-подписью сервера, деление при выдаче, возврат, повтор оплаты.

use orders_escrow::{EscrowError, SEED_VAULT, config, ix, order, status};
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use swarm_test_utils::*;

const PRICE: u64 = 500_000; // 0.5 токена при 6 знаках, как цена действия 
const GRACE: i64 = 3 * 86_400;

fn pid() -> Pubkey {
    Pubkey::new_from_array(orders_escrow::ID.to_bytes())
}
fn cfg() -> Pubkey {
    Pubkey::find_program_address(&[config::SEED], &pid()).0
}
fn vault() -> Pubkey {
    Pubkey::find_program_address(&[SEED_VAULT], &pid()).0
}
fn order_pda(id: &[u8; 16]) -> Pubkey {
    Pubkey::find_program_address(&[order::SEED, id], &pid()).0
}

struct W {
    env: Env,
    operator: Keypair,
    mint: Pubkey,
    payout: Pubkey,
    treasury: Pubkey,
}

impl W {
    fn new() -> Self {
        let mut env = Env::new(&[(pid(), "orders_escrow")]);
        let admin = env.funded();
        let operator = env.funded();
        let mint = env.create_mint(&admin.pubkey(), 6);
        let payout = env.create_token_account(&mint, &admin.pubkey(), 0);
        let treasury = env.create_token_account(&mint, &admin.pubkey(), 0);
        let mut data = vec![ix::INITIALIZE];
        data.extend_from_slice(&1_500u16.to_le_bytes()); // 15% сжигание
        data.extend_from_slice(&1_000u16.to_le_bytes()); // 10% казна
        data.extend_from_slice(&GRACE.to_le_bytes());
        let i = Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new(admin.pubkey(), true),
                AccountMeta::new(cfg(), false),
                AccountMeta::new(vault(), false),
                AccountMeta::new_readonly(mint, false),
                AccountMeta::new_readonly(operator.pubkey(), false),
                AccountMeta::new_readonly(payout, false),
                AccountMeta::new_readonly(treasury, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data,
        };
        env.send(&[i], &[&admin]).expect("initialize");
        Self { env, operator, mint, payout, treasury }
    }

    fn pay_ix(&self, payer: &Pubkey, payer_token: &Pubkey, id: [u8; 16], amount: u64, deadline: i64) -> Instruction {
        let mut data = vec![ix::PAY];
        data.extend_from_slice(&id);
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&deadline.to_le_bytes());
        Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new_readonly(*payer, true),
                AccountMeta::new(self.operator.pubkey(), true),
                AccountMeta::new_readonly(cfg(), false),
                AccountMeta::new(order_pda(&id), false),
                AccountMeta::new(*payer_token, false),
                AccountMeta::new(vault(), false),
                AccountMeta::new_readonly(self.mint, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data,
        }
    }

    fn release_ix(&self, signer: &Pubkey, id: &[u8; 16]) -> Instruction {
        Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new_readonly(*signer, true),
                AccountMeta::new(cfg(), false),
                AccountMeta::new(order_pda(id), false),
                AccountMeta::new(vault(), false),
                AccountMeta::new(self.payout, false),
                AccountMeta::new(self.treasury, false),
                AccountMeta::new(self.mint, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![ix::RELEASE],
        }
    }

    fn refund_ix(&self, caller: &Pubkey, id: &[u8; 16], payer_token: &Pubkey) -> Instruction {
        Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new_readonly(*caller, true),
                AccountMeta::new_readonly(cfg(), false),
                AccountMeta::new(order_pda(id), false),
                AccountMeta::new(vault(), false),
                AccountMeta::new(*payer_token, false),
                AccountMeta::new_readonly(self.mint, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![ix::REFUND],
        }
    }
}

#[test]
fn pay_then_release_splits_65_10_15_10() {
    let mut w = W::new();
    let buyer = w.env.funded();
    let ata = w.env.create_token_account(&w.mint, &buyer.pubkey(), PRICE * 3);
    let id = [1u8; 16];
    let deadline = w.env.now() + 3_600;
    let op = w.operator.insecure_clone();

    let cu = w.env.send(&[w.pay_ix(&buyer.pubkey(), &ata, id, PRICE, deadline)], &[&buyer, &op]).unwrap();
    println!("pay: {cu} CU");
    assert_eq!(w.env.token_balance(&vault()), PRICE);
    assert_eq!(w.env.data(&order_pda(&id))[order::STATUS], status::FUNDED);

    // повтор той же оплаты не списывает второй раз
    assert!(w.env.send(&[w.pay_ix(&buyer.pubkey(), &ata, id, PRICE, deadline)], &[&buyer, &op]).is_err());
    assert_eq!(w.env.token_balance(&ata), PRICE * 2);

    // выдавать может только оператор
    assert_eq!(
        w.env.send(&[w.release_ix(&buyer.pubkey(), &id)], &[&buyer]).unwrap_err(),
        custom_err(0, EscrowError::Unauthorized as u32)
    );
    let cu = w.env.send(&[w.release_ix(&op.pubkey(), &id)], &[&op]).unwrap();
    println!("release: {cu} CU");
    assert_eq!(w.env.token_balance(&w.treasury), PRICE / 10);
    assert_eq!(w.env.token_balance(&w.payout), PRICE * 75 / 100);
    assert_eq!(w.env.token_balance(&vault()), 0);
    let c = w.env.data(&cfg());
    assert_eq!(read_u64(&c, config::TOTAL_BURNED), PRICE * 15 / 100);
    assert_eq!(read_u64(&c, config::TOTAL_PAID), PRICE);

    // второй раз не выдать, а закрыть заказ можно — рента вернётся оператору
    assert_eq!(
        w.env.send(&[w.release_ix(&op.pubkey(), &id)], &[&op]).unwrap_err(),
        custom_err(0, EscrowError::BadStatus as u32)
    );
    let close = Instruction {
        program_id: pid(),
        accounts: vec![AccountMeta::new(order_pda(&id), false), AccountMeta::new(op.pubkey(), false)],
        data: vec![ix::CLOSE_ORDER],
    };
    w.env.send(&[close], &[]).unwrap();
    assert!(!w.env.exists(&order_pda(&id)));
}

#[test]
fn pay_requires_operator_cosign() {
    let mut w = W::new();
    let buyer = w.env.funded();
    let fake_op = w.env.funded();
    let ata = w.env.create_token_account(&w.mint, &buyer.pubkey(), PRICE);
    let id = [2u8; 16];
    let mut i = w.pay_ix(&buyer.pubkey(), &ata, id, PRICE, w.env.now() + 60);
    i.accounts[1] = AccountMeta::new(fake_op.pubkey(), true);
    assert_eq!(w.env.send(&[i], &[&buyer, &fake_op]).unwrap_err(), custom_err(0, EscrowError::Unauthorized as u32));
}

#[test]
fn refund_by_operator_or_by_anyone_after_grace() {
    let mut w = W::new();
    let buyer = w.env.funded();
    let ata = w.env.create_token_account(&w.mint, &buyer.pubkey(), PRICE * 2);
    let op = w.operator.insecure_clone();
    let deadline = w.env.now() + 3_600;

    // оператор возвращает сразу
    let a = [3u8; 16];
    w.env.send(&[w.pay_ix(&buyer.pubkey(), &ata, a, PRICE, deadline)], &[&buyer, &op]).unwrap();
    w.env.send(&[w.refund_ix(&op.pubkey(), &a, &ata)], &[&op]).unwrap();
    assert_eq!(w.env.token_balance(&ata), PRICE * 2);
    assert_eq!(w.env.data(&order_pda(&a))[order::STATUS], status::REFUNDED);

    // заказчик сам — только после deadline + grace
    let b = [4u8; 16];
    w.env.send(&[w.pay_ix(&buyer.pubkey(), &ata, b, PRICE, deadline)], &[&buyer, &op]).unwrap();
    assert_eq!(
        w.env.send(&[w.refund_ix(&buyer.pubkey(), &b, &ata)], &[&buyer]).unwrap_err(),
        custom_err(0, EscrowError::RefundNotAllowed as u32)
    );
    w.env.warp_seconds(3_600 + GRACE + 1);
    w.env.send(&[w.refund_ix(&buyer.pubkey(), &b, &ata)], &[&buyer]).unwrap();
    assert_eq!(w.env.token_balance(&ata), PRICE * 2);

    // вернуть можно только на исходный аккаунт плательщика
    let c = [5u8; 16];
    let deadline = w.env.now() + 60;
    w.env.send(&[w.pay_ix(&buyer.pubkey(), &ata, c, PRICE, deadline)], &[&buyer, &op]).unwrap();
    let thief = w.env.create_token_account(&w.mint, &op.pubkey(), 0);
    assert_eq!(
        w.env.send(&[w.refund_ix(&op.pubkey(), &c, &thief)], &[&op]).unwrap_err(),
        custom_err(0, EscrowError::WrongAccount as u32)
    );
}
