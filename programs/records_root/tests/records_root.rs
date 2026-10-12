//! Публикация суточного корня и проверка доказательств записей.

use records_root::{RootError, config, day, ix};
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use swarm_test_utils::{merkle, *};

fn pid() -> Pubkey {
    Pubkey::new_from_array(records_root::ID.to_bytes())
}
fn cfg() -> Pubkey {
    Pubkey::find_program_address(&[config::SEED], &pid()).0
}
fn day_pda(d: u32) -> Pubkey {
    Pubkey::find_program_address(&[day::SEED, &d.to_le_bytes()], &pid()).0
}

fn publish_ix(writer: &Pubkey, d: u32, root: [u8; 32], count: u32) -> Instruction {
    let mut data = vec![ix::PUBLISH];
    data.extend_from_slice(&d.to_le_bytes());
    data.extend_from_slice(&root);
    data.extend_from_slice(&count.to_le_bytes());
    Instruction {
        program_id: pid(),
        accounts: vec![
            AccountMeta::new(*writer, true),
            AccountMeta::new(cfg(), false),
            AccountMeta::new(day_pda(d), false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data,
    }
}

fn verify_ix(d: u32, job: [u8; 16], doc: [u8; 32], proof: &[[u8; 32]]) -> Instruction {
    let mut data = vec![ix::VERIFY];
    data.extend_from_slice(&job);
    data.extend_from_slice(&doc);
    data.extend_from_slice(&(proof.len() as u32).to_le_bytes());
    for p in proof {
        data.extend_from_slice(p);
    }
    Instruction { program_id: pid(), accounts: vec![AccountMeta::new_readonly(day_pda(d), false)], data }
}

fn setup() -> (Env, Keypair) {
    let mut env = Env::new(&[(pid(), "records_root")]);
    let admin = env.funded();
    let writer = env.funded();
    let i = Instruction {
        program_id: pid(),
        accounts: vec![
            AccountMeta::new(admin.pubkey(), true),
            AccountMeta::new(cfg(), false),
            AccountMeta::new_readonly(writer.pubkey(), false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data: vec![ix::INITIALIZE],
    };
    env.send(&[i], &[&admin]).unwrap();
    (env, writer)
}

#[test]
fn publish_daily_root_and_verify_records() {
    let (mut env, writer) = setup();
    env.warp_seconds(1_790_000_000); // LiteSVM стартует с unix_timestamp = 0
    let today = (env.now() / 86_400) as u32;
    let records: Vec<([u8; 16], [u8; 32])> = (0..9u8).map(|i| ([i; 16], [i ^ 0xaa; 32])).collect();
    let leaves: Vec<[u8; 32]> = records.iter().map(|(j, d)| merkle::record_leaf(j, d)).collect();
    let (root, layers) = merkle::build(&leaves);

    // за сегодняшний (не закрытый) день нельзя
    assert_eq!(
        env.send(&[publish_ix(&writer.pubkey(), today, root, 9)], &[&writer]).unwrap_err(),
        custom_err(0, RootError::FutureDay as u32)
    );
    let yesterday = today - 1;
    env.send(&[publish_ix(&writer.pubkey(), yesterday, root, 9)], &[&writer]).unwrap();
    // второй раз тот же день не переписать
    assert!(env.send(&[publish_ix(&writer.pubkey(), yesterday, [0u8; 32], 1)], &[&writer]).is_err());

    for (i, (j, d)) in records.iter().enumerate() {
        let cu = env.send(&[verify_ix(yesterday, *j, *d, &merkle::proof(&layers, &leaves[i]))], &[]).unwrap();
        if i == 0 {
            println!("verify: {cu} CU");
        }
    }
    // подменённый documentHash не проходит
    let (j, _) = records[0];
    assert_eq!(
        env.send(&[verify_ix(yesterday, j, [0u8; 32], &merkle::proof(&layers, &leaves[0]))], &[]).unwrap_err(),
        custom_err(0, RootError::InvalidProof as u32)
    );
    assert_eq!(read_u64(&env.data(&cfg()), config::DAYS), 1);

    // публиковать может только writer
    let stranger = env.funded();
    assert_eq!(
        env.send(&[publish_ix(&stranger.pubkey(), yesterday - 1, root, 9)], &[&stranger]).unwrap_err(),
        custom_err(0, RootError::Unauthorized as u32)
    );
}
