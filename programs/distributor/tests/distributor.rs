//! Merkle-claim: забор кумулятивного баланса, повторный забор только разницы, автозабор,
//! чужое доказательство и старые эпохи отклоняются.

use distributor::{DistError, SEED_VAULT, claim, dist, ix};
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use swarm_test_utils::{merkle, *};

fn pid() -> Pubkey {
    Pubkey::new_from_array(distributor::ID.to_bytes())
}
fn dist_pda(id: u64) -> Pubkey {
    Pubkey::find_program_address(&[dist::SEED, &id.to_le_bytes()], &pid()).0
}
fn vault_pda(d: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[SEED_VAULT, d.as_ref()], &pid()).0
}
fn claim_pda(d: &Pubkey, wallet: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[claim::SEED, d.as_ref(), wallet.as_ref()], &pid()).0
}

struct W {
    env: Env,
    updater: Keypair,
    mint: Pubkey,
    d: Pubkey,
}

impl W {
    fn new() -> Self {
        let mut env = Env::new(&[(pid(), "distributor")]);
        let admin = env.funded();
        let updater = env.funded();
        let mint = env.create_mint(&admin.pubkey(), 6);
        let d = dist_pda(0);
        let mut data = vec![ix::CREATE];
        data.extend_from_slice(&0u64.to_le_bytes());
        let i = Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new(admin.pubkey(), true),
                AccountMeta::new(d, false),
                AccountMeta::new(vault_pda(&d), false),
                AccountMeta::new_readonly(mint, false),
                AccountMeta::new_readonly(updater.pubkey(), false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data,
        };
        env.send(&[i], &[&admin]).expect("create");
        // пополняем хранилище (в проде сюда приходит доля из orders_escrow)
        env.set_token_account(vault_pda(&d), &mint, &d, 1_000_000);
        Self { env, updater, mint, d }
    }

    fn set_root(&mut self, root: [u8; 32], epoch: u64) -> Result<u64, TransactionError> {
        let mut data = vec![ix::SET_ROOT];
        data.extend_from_slice(&root);
        data.extend_from_slice(&epoch.to_le_bytes());
        let i = Instruction {
            program_id: pid(),
            accounts: vec![AccountMeta::new_readonly(self.updater.pubkey(), true), AccountMeta::new(self.d, false)],
            data,
        };
        let u = self.updater.insecure_clone();
        self.env.send(&[i], &[&u])
    }

    fn claim_ix(&self, signer: &Pubkey, wallet: &Pubkey, wallet_token: &Pubkey, cumulative: u64, proof: &[[u8; 32]]) -> Instruction {
        let mut data = vec![ix::CLAIM];
        data.extend_from_slice(&cumulative.to_le_bytes());
        data.extend_from_slice(&(proof.len() as u32).to_le_bytes());
        for p in proof {
            data.extend_from_slice(p);
        }
        Instruction {
            program_id: pid(),
            accounts: vec![
                AccountMeta::new(*signer, true),
                AccountMeta::new_readonly(*wallet, false),
                AccountMeta::new(self.d, false),
                AccountMeta::new(claim_pda(&self.d, wallet), false),
                AccountMeta::new(vault_pda(&self.d), false),
                AccountMeta::new(*wallet_token, false),
                AccountMeta::new_readonly(self.mint, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data,
        }
    }
}

#[test]
fn cumulative_claims_across_epochs() {
    let mut w = W::new();
    let alice = w.env.funded();
    let bob = w.env.funded();
    let a_tok = w.env.create_token_account(&w.mint, &alice.pubkey(), 0);
    let b_tok = w.env.create_token_account(&w.mint, &bob.pubkey(), 0);
    let others: Vec<[u8; 32]> = (0..5u8).map(|i| merkle::claim_leaf(&[i + 10; 32], 1)).collect();

    // эпоха 1: alice 100, bob 50
    let la = merkle::claim_leaf(&alice.pubkey().to_bytes(), 100);
    let lb = merkle::claim_leaf(&bob.pubkey().to_bytes(), 50);
    let mut leaves = vec![la, lb];
    leaves.extend(&others);
    let (root, layers) = merkle::build(&leaves);
    w.set_root(root, 1).unwrap();

    let pa = merkle::proof(&layers, &la);
    let cu = w.env.send(&[w.claim_ix(&alice.pubkey(), &alice.pubkey(), &a_tok, 100, &pa)], &[&alice]).unwrap();
    println!("claim: {cu} CU");
    assert_eq!(w.env.token_balance(&a_tok), 100);
    // повтор — нечего забирать
    assert_eq!(
        w.env.send(&[w.claim_ix(&alice.pubkey(), &alice.pubkey(), &a_tok, 100, &pa)], &[&alice]).unwrap_err(),
        custom_err(0, DistError::NothingToClaim as u32)
    );
    // alice не может забрать сумму bob по его доказательству
    let pb = merkle::proof(&layers, &lb);
    assert_eq!(
        w.env.send(&[w.claim_ix(&alice.pubkey(), &alice.pubkey(), &a_tok, 50, &pb)], &[&alice]).unwrap_err(),
        custom_err(0, DistError::InvalidProof as u32)
    );

    // эпоха 2: alice накопила 250 — получает только разницу 150; bob забирает автозабором
    let la2 = merkle::claim_leaf(&alice.pubkey().to_bytes(), 250);
    let lb2 = merkle::claim_leaf(&bob.pubkey().to_bytes(), 80);
    let mut leaves = vec![la2, lb2];
    leaves.extend(&others);
    let (root2, layers2) = merkle::build(&leaves);
    assert_eq!(w.set_root(root2, 1).unwrap_err(), custom_err(0, DistError::StaleEpoch as u32));
    w.set_root(root2, 2).unwrap();
    w.env.send(&[w.claim_ix(&alice.pubkey(), &alice.pubkey(), &a_tok, 250, &merkle::proof(&layers2, &la2))], &[&alice]).unwrap();
    assert_eq!(w.env.token_balance(&a_tok), 250);

    let u = w.updater.insecure_clone();
    w.env.send(&[w.claim_ix(&u.pubkey(), &bob.pubkey(), &b_tok, 80, &merkle::proof(&layers2, &lb2))], &[&u]).unwrap();
    assert_eq!(w.env.token_balance(&b_tok), 80);
    assert_eq!(read_u64(&w.env.data(&w.d), dist::TOTAL_CLAIMED), 330);

    // автозабор не может увести на чужой аккаунт
    let carol = w.env.funded();
    let lc = merkle::claim_leaf(&carol.pubkey().to_bytes(), 10);
    let (root3, layers3) = merkle::build(&[la2, lb2, lc]);
    w.set_root(root3, 3).unwrap();
    let u_tok = w.env.create_token_account(&w.mint, &u.pubkey(), 0);
    assert_eq!(
        w.env.send(&[w.claim_ix(&u.pubkey(), &carol.pubkey(), &u_tok, 10, &merkle::proof(&layers3, &lc))], &[&u]).unwrap_err(),
        custom_err(0, DistError::WrongAccount as u32)
    );
    // посторонний не может забрать за carol
    let mallory = w.env.funded();
    let c_tok = w.env.create_token_account(&w.mint, &carol.pubkey(), 0);
    assert_eq!(
        w.env.send(&[w.claim_ix(&mallory.pubkey(), &carol.pubkey(), &c_tok, 10, &merkle::proof(&layers3, &lc))], &[&mallory]).unwrap_err(),
        custom_err(0, DistError::Unauthorized as u32)
    );
}
