//! Деление комиссий 50/20/10/10/10 со сжиганием; кто угодно вызывает distribute; доли = 100%.

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use swarm_test_utils::*;
use treasury_split::{BURN, SEED_VAULT, SplitError, ix, split};

fn pid() -> Pubkey {
    Pubkey::new_from_array(treasury_split::ID.to_bytes())
}
fn split_pda() -> Pubkey {
    Pubkey::find_program_address(&[split::SEED], &pid()).0
}
fn vault() -> Pubkey {
    Pubkey::find_program_address(&[SEED_VAULT], &pid()).0
}

fn recipients_data(list: &[([u8; 32], u16)]) -> Vec<u8> {
    let mut d = (list.len() as u32).to_le_bytes().to_vec();
    for (dest, bps) in list {
        d.extend_from_slice(dest);
        d.extend_from_slice(&bps.to_le_bytes());
    }
    d
}

#[test]
fn distributes_by_shares_and_burns() {
    let mut env = Env::new(&[(pid(), "treasury_split")]);
    let admin = env.funded();
    let mint = env.create_mint(&admin.pubkey(), 6);
    let work = env.create_token_account(&mint, &admin.pubkey(), 0);
    let presence = env.create_token_account(&mint, &admin.pubkey(), 0);
    let treasury = env.create_token_account(&mint, &admin.pubkey(), 0);
    let orchestrator = env.create_token_account(&mint, &admin.pubkey(), 0);
    let shares = [
        (BURN, 5_000u16),
        (work.to_bytes(), 2_000),
        (presence.to_bytes(), 1_000),
        (treasury.to_bytes(), 1_000),
        (orchestrator.to_bytes(), 1_000),
    ];
    let init = |list: &[([u8; 32], u16)]| {
        let mut d = vec![ix::INITIALIZE];
        d.extend(recipients_data(list));
        Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new(admin.pubkey(), true),
                AccountMeta::new(split_pda(), false),
                AccountMeta::new(vault(), false),
                AccountMeta::new_readonly(mint, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: d,
        }
    };
    // доли должны давать ровно 100%
    let mut bad = shares;
    bad[0].1 = 4_999;
    assert_eq!(env.send(&[init(&bad)], &[&admin]).unwrap_err(), custom_err(0, SplitError::BadShares as u32));
    env.send(&[init(&shares)], &[&admin]).unwrap();

    let distribute = Instruction {
        program_id: pid(),
        accounts: vec![
            AccountMeta::new(split_pda(), false),
            AccountMeta::new(vault(), false),
            AccountMeta::new(mint, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new(work, false),
            AccountMeta::new(presence, false),
            AccountMeta::new(treasury, false),
            AccountMeta::new(orchestrator, false),
        ],
        data: vec![ix::DISTRIBUTE],
    };
    assert_eq!(env.send(&[distribute.clone()], &[]).unwrap_err(), custom_err(0, SplitError::NothingToDistribute as u32));

    // пришли комиссии — распределяет любой
    env.set_token_account(vault(), &mint, &split_pda(), 1_000_000);
    let caller = env.funded();
    let cu = env.send(&[distribute.clone()], &[]).unwrap();
    println!("distribute(5): {cu} CU");
    assert_eq!(env.token_balance(&work), 200_000);
    assert_eq!(env.token_balance(&presence), 100_000);
    assert_eq!(env.token_balance(&treasury), 100_000);
    assert_eq!(env.token_balance(&orchestrator), 100_000);
    assert_eq!(env.token_balance(&vault()), 0, "50% сожжено");
    assert_eq!(read_u64(&env.data(&split_pda()), split::TOTAL_DISTRIBUTED), 1_000_000);

    // подмена получателя ловится
    env.set_token_account(vault(), &mint, &split_pda(), 1_000);
    let thief = env.create_token_account(&mint, &caller.pubkey(), 0);
    let mut bad_dist = distribute;
    bad_dist.accounts[4] = AccountMeta::new(thief, false);
    assert_eq!(env.send(&[bad_dist], &[]).unwrap_err(), custom_err(0, SplitError::WrongAccount as u32));
}
