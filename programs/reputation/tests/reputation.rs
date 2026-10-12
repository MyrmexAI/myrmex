//! Пачки квитанций: последовательные номера, содержимое, права writer.

use reputation::{ReputationError, batch, config, ix};
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use swarm_test_utils::*;

fn pid() -> Pubkey {
    Pubkey::new_from_array(reputation::ID.to_bytes())
}
fn cfg() -> Pubkey {
    Pubkey::find_program_address(&[config::SEED], &pid()).0
}
fn batch_pda(seq: u64) -> Pubkey {
    Pubkey::find_program_address(&[batch::SEED, &seq.to_le_bytes()], &pid()).0
}
fn submit_ix(writer: &Pubkey, seq: u64, entries: &[([u8; 32], [u8; 32])]) -> Instruction {
    let mut data = vec![ix::SUBMIT_BATCH];
    data.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (s, h) in entries {
        data.extend_from_slice(s);
        data.extend_from_slice(h);
    }
    Instruction {
        program_id: pid(),
        accounts: vec![
            AccountMeta::new(*writer, true),
            AccountMeta::new(cfg(), false),
            AccountMeta::new(batch_pda(seq), false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data,
    }
}

#[test]
fn batches_are_sequential_and_store_receipts() {
    let mut env = Env::new(&[(pid(), "reputation")]);
    let admin = env.funded();
    let writer = env.funded();
    let init = Instruction {
        program_id: pid(),
        accounts: vec![
            AccountMeta::new(admin.pubkey(), true),
            AccountMeta::new(cfg(), false),
            AccountMeta::new_readonly(writer.pubkey(), false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data: vec![ix::INITIALIZE],
    };
    env.send(&[init], &[&admin]).unwrap();

    let entries: Vec<([u8; 32], [u8; 32])> = (0..12u8).map(|i| ([i; 32], [255 - i; 32])).collect();
    let cu = env.send(&[submit_ix(&writer.pubkey(), 0, &entries)], &[&writer]).unwrap();
    println!("submit_batch(12): {cu} CU");
    let d = env.data(&batch_pda(0));
    assert_eq!(read_u16(&d, batch::COUNT), 12);
    assert_eq!(&d[batch::ENTRIES + 64 * 5..batch::ENTRIES + 64 * 5 + 32], &[5u8; 32]);
    assert_eq!(&d[batch::ENTRIES + 64 * 5 + 32..batch::ENTRIES + 64 * 6], &[250u8; 32]);

    // номер пачки берётся из конфига: пропустить или повторить нельзя
    assert!(env.send(&[submit_ix(&writer.pubkey(), 5, &entries[..1])], &[&writer]).is_err());
    assert!(env.send(&[submit_ix(&writer.pubkey(), 0, &entries[..1])], &[&writer]).is_err());
    env.send(&[submit_ix(&writer.pubkey(), 1, &entries[..1])], &[&writer]).unwrap();
    let c = env.data(&cfg());
    assert_eq!(read_u64(&c, config::NEXT_SEQ), 2);
    assert_eq!(read_u64(&c, config::TOTAL_RECEIPTS), 13);

    let stranger = env.funded();
    assert_eq!(
        env.send(&[submit_ix(&stranger.pubkey(), 2, &entries[..1])], &[&stranger]).unwrap_err(),
        custom_err(0, ReputationError::Unauthorized as u32)
    );
    assert_eq!(
        env.send(&[submit_ix(&writer.pubkey(), 2, &[])], &[&writer]).unwrap_err(),
        custom_err(0, ReputationError::BadBatchSize as u32)
    );
}
