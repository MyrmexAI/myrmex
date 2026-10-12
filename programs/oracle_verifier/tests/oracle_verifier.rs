//! Проверка ответа оракула: Ed25519 program + verify; чужой ключ, подмена байт, истёкший срок,
//! чужой потребитель и слабая панель отклоняются.

use ed25519_dalek::{Signer as _, SigningKey};
use oracle_verifier::{DOMAIN, OracleError, config, ix};
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use swarm_test_utils::*;

fn pid() -> Pubkey {
    Pubkey::new_from_array(oracle_verifier::ID.to_bytes())
}
fn cfg() -> Pubkey {
    Pubkey::find_program_address(&[config::SEED], &pid()).0
}
const ED25519: Pubkey = Pubkey::from_str_const("Ed25519SigVerify111111111111111111111111111");
const IX_SYSVAR: Pubkey = Pubkey::from_str_const("Sysvar1nstructions1111111111111111111111111");

/// Borsh-сообщение OracleAttestation с доменом и потребителем.
fn attestation(consumer: &Pubkey, answer: &[u8], panel_size: u16, quorum: u16, agreed: u16, expires_at: i64) -> Vec<u8> {
    let mut m = DOMAIN.to_vec();
    m.extend_from_slice(consumer.as_ref());
    m.extend_from_slice(&[7u8; 16]); // requestId
    m.push(1); // cluster: devnet
    m.extend_from_slice(&[9u8; 32]); // questionHash
    m.push(0); // answerType: bool
    m.extend_from_slice(&(answer.len() as u32).to_le_bytes());
    m.extend_from_slice(answer);
    m.extend_from_slice(&0u128.to_le_bytes()); // figure
    m.extend_from_slice(&100u64.to_le_bytes()); // fromSlot
    m.extend_from_slice(&200u64.to_le_bytes()); // toSlot
    m.extend_from_slice(&[3u8; 32]); // slotHash
    m.extend_from_slice(&[4u8; 16]); // panelJobId
    m.extend_from_slice(&panel_size.to_le_bytes());
    m.extend_from_slice(&quorum.to_le_bytes());
    m.extend_from_slice(&agreed.to_le_bytes());
    m.extend_from_slice(&1_790_000_000i64.to_le_bytes()); // issuedAt
    m.extend_from_slice(&expires_at.to_le_bytes());
    m
}

/// Инструкция Ed25519 program: подпись, ключ и сообщение внутри неё самой.
fn ed25519_ix(key: &SigningKey, msg: &[u8]) -> Instruction {
    let sig = key.sign(msg).to_bytes();
    let pk = key.verifying_key().to_bytes();
    let (pk_off, sig_off, msg_off) = (16u16, 48u16, 112u16);
    let mut d = vec![1u8, 0];
    for v in [sig_off, u16::MAX, pk_off, u16::MAX, msg_off, msg.len() as u16, u16::MAX] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d.extend_from_slice(&pk);
    d.extend_from_slice(&sig);
    d.extend_from_slice(msg);
    Instruction { program_id: ED25519, accounts: vec![], data: d }
}

fn verify_ix(consumer: &Pubkey, min_panel: u16, min_agreed: u16, msg: &[u8]) -> Instruction {
    let mut d = vec![ix::VERIFY];
    d.extend_from_slice(consumer.as_ref());
    d.extend_from_slice(&min_panel.to_le_bytes());
    d.extend_from_slice(&min_agreed.to_le_bytes());
    d.extend_from_slice(&(msg.len() as u32).to_le_bytes());
    d.extend_from_slice(msg);
    Instruction {
        program_id: pid(),
        accounts: vec![AccountMeta::new_readonly(cfg(), false), AccountMeta::new_readonly(IX_SYSVAR, false)],
        data: d,
    }
}

#[test]
fn attestation_checks() {
    // Ed25519 program в LiteSVM без precompiles заменена заглушкой: здесь проверяется разбор и
    // сверка подписанных байт в oracle_verifier; саму подпись в сети проверяет настоящая Ed25519 program.
    let mut env = Env::new(&[(pid(), "oracle_verifier"), (ED25519, "test_noop")]);
    env.warp_seconds(1_790_000_000);
    let admin = env.funded();
    let attester = SigningKey::from_bytes(&[42u8; 32]);
    let mut d = vec![ix::INITIALIZE];
    d.extend_from_slice(&attester.verifying_key().to_bytes());
    let init = Instruction {
        program_id: pid(),
        accounts: vec![
            AccountMeta::new(admin.pubkey(), true),
            AccountMeta::new(cfg(), false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data: d,
    };
    env.send(&[init], &[&admin]).unwrap();

    let consumer = Pubkey::new_unique();
    let exp = env.now() + 3_600;
    let msg = attestation(&consumer, &[1], 5, 4, 5, exp);

    let cu = env.send(&[ed25519_ix(&attester, &msg), verify_ix(&consumer, 5, 4, &msg)], &[]).unwrap();
    println!("ed25519 + verify: {cu} CU");

    // без инструкции Ed25519 перед verify
    assert_eq!(env.send(&[verify_ix(&consumer, 5, 4, &msg)], &[]).unwrap_err(), custom_err(0, OracleError::MissingSignature as u32));
    // подписано чужим ключом
    let rogue = SigningKey::from_bytes(&[1u8; 32]);
    assert_eq!(
        env.send(&[ed25519_ix(&rogue, &msg), verify_ix(&consumer, 5, 4, &msg)], &[]).unwrap_err(),
        custom_err(1, OracleError::WrongAttester as u32)
    );
    // подпись над одним ответом, а в verify подсунут другой
    let forged = attestation(&consumer, &[0], 5, 4, 5, exp);
    assert_eq!(
        env.send(&[ed25519_ix(&attester, &msg), verify_ix(&consumer, 5, 4, &forged)], &[]).unwrap_err(),
        custom_err(1, OracleError::MessageMismatch as u32)
    );
    // ответ для другого потребителя
    let other = Pubkey::new_unique();
    assert_eq!(
        env.send(&[ed25519_ix(&attester, &msg), verify_ix(&other, 5, 4, &msg)], &[]).unwrap_err(),
        custom_err(1, OracleError::WrongConsumer as u32)
    );
    // требования потребителя к панели
    assert_eq!(
        env.send(&[ed25519_ix(&attester, &msg), verify_ix(&consumer, 7, 4, &msg)], &[]).unwrap_err(),
        custom_err(1, OracleError::PanelTooSmall as u32)
    );
    assert_eq!(
        env.send(&[ed25519_ix(&attester, &msg), verify_ix(&consumer, 5, 6, &msg)], &[]).unwrap_err(),
        custom_err(1, OracleError::NotEnoughAgreement as u32)
    );
    // срок действия истёк
    env.warp_seconds(3_601);
    assert_eq!(
        env.send(&[ed25519_ix(&attester, &msg), verify_ix(&consumer, 5, 4, &msg)], &[]).unwrap_err(),
        custom_err(1, OracleError::Expired as u32)
    );
}
