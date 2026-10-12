//! `oracle_verifier` — проверка подписанных ответов оракула.
//!
//! Ответ оракула — Borsh-сообщение `OracleAttestation`, подписанное Ed25519-ключом аттестатора.
//! Подпись проверяет нативная Ed25519 program в той же транзакции (инструкция прямо перед `verify`);
//! эта программа через introspection (`sysvar::instructions`) убеждается, что проверялись именно
//! наш ключ аттестатора и именно эти байты, затем проверяет домен, срок действия и требования
//! потребителя (`min_panel_size`, `min_agreed`). Программа-потребитель вызывает `verify` через CPI
//! или требует его в той же транзакции.
//!
//! Подписываемое сообщение: `DOMAIN (16 байт) || consumer_program (32) || attestation (Borsh)`.
//! attestation: requestId[16] | cluster u8 | questionHash[32] | answerType u8 | answer: u32 len + bytes |
//! figure u128 | fromSlot u64 | toSlot u64 | slotHash[32] | panelJobId[16] | panelSize u16 | quorum u16 |
//! agreed u16 | issuedAt i64 | expiresAt i64.

use pinocchio::{AccountView, Address, ProgramResult, cpi::Seed, error::ProgramError, sysvars::instructions::Instructions};
use shank::ShankInstruction;
use swarm_program_utils::*;

solana_address::declare_id!("7WfNVUeyBU4JiArwS3VQTGzm4mQUc3NbKRT18MCMQ4oy");

#[cfg(target_os = "solana")]
pinocchio::entrypoint!(process_instruction);

pub const DOMAIN: &[u8; 16] = b"swarm.oracle.v1\0";
pub const ED25519_PROGRAM: Address = Address::from_str_const("Ed25519SigVerify111111111111111111111111111");

pub mod config {
    pub const DISC: u8 = 1;
    pub const SEED: &[u8] = b"config";
    pub const BUMP: usize = 2;
    pub const ADMIN: usize = 8;
    pub const ATTESTER: usize = 40; // Ed25519 pubkey аттестатора
    pub const LEN: usize = 72;
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OracleError {
    Unauthorized = 0,
    MissingSignature = 1,
    WrongAttester = 2,
    MessageMismatch = 3,
    WrongDomain = 4,
    WrongConsumer = 5,
    Expired = 6,
    PanelTooSmall = 7,
    NotEnoughAgreement = 8,
    Malformed = 9,
}

impl From<OracleError> for ProgramError {
    fn from(e: OracleError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

#[derive(ShankInstruction)]
#[rustfmt::skip]
pub enum OracleInstruction {
    #[account(0, writable, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    #[account(2, name = "system_program")]
    Initialize { attester: [u8; 32] },

    #[account(0, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    SetAttester { admin: [u8; 32], attester: [u8; 32] },

    #[account(0, name = "config")]
    #[account(1, name = "instructions_sysvar")]
    Verify { consumer: [u8; 32], min_panel_size: u16, min_agreed: u16, message: Vec<u8> },
}

pub mod ix {
    pub const INITIALIZE: u8 = 0;
    pub const SET_ATTESTER: u8 = 1;
    pub const VERIFY: u8 = 2;
}

/// Смещения полей после `answer` считаются от конца сообщения (хвост фиксированной длины).
pub mod tail {
    pub const LEN: usize = 16 + 8 + 8 + 32 + 16 + 2 + 2 + 2 + 8 + 8; // 102
    pub const PANEL_SIZE: usize = LEN - 22; // от начала хвоста
    pub const AGREED: usize = LEN - 18;
    pub const EXPIRES_AT: usize = LEN - 8;
}

pub fn process_instruction(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let (disc, rest) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    let mut r = Reader::new(rest);
    match *disc {
        ix::INITIALIZE => initialize(program_id, accounts, &mut r),
        ix::SET_ATTESTER => set_attester(program_id, accounts, &mut r),
        ix::VERIFY => verify(program_id, accounts, &mut r),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn initialize(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let attester = r.bytes32()?;
    r.finish()?;
    let [admin, cfg, system_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_system_program(system_program)?;
    let (key, bump) = find_pda(&[config::SEED], program_id);
    require_key(cfg, &key)?;
    if !cfg.is_data_empty() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let b = [bump];
    create_pda_account(admin, cfg, config::LEN, program_id, &[Seed::from(config::SEED), Seed::from(&b)])?;
    let mut d = cfg.try_borrow_mut()?;
    d[0] = config::DISC;
    d[1] = 1;
    d[config::BUMP] = bump;
    write_bytes(&mut d, config::ADMIN, admin.address().as_array());
    write_bytes(&mut d, config::ATTESTER, &attester);
    Ok(())
}

fn set_attester(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let admin_new = r.bytes32()?;
    let attester = r.bytes32()?;
    r.finish()?;
    let [admin, cfg, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_state(cfg, program_id, config::DISC, config::LEN)?;
    let mut d = cfg.try_borrow_mut()?;
    if read_addr(&d, config::ADMIN) != *admin.address() {
        return Err(OracleError::Unauthorized.into());
    }
    write_bytes(&mut d, config::ADMIN, &admin_new);
    write_bytes(&mut d, config::ATTESTER, &attester);
    Ok(())
}

/// Разбор данных инструкции Ed25519 program с одной подписью, где подпись, ключ и сообщение
/// лежат в самой инструкции (индексы инструкций = u16::MAX). Возвращает (pubkey, message).
fn parse_ed25519(data: &[u8]) -> Result<(&[u8], &[u8]), ProgramError> {
    if data.len() < 16 || data[0] != 1 {
        return Err(OracleError::MissingSignature.into());
    }
    let u16_at = |o: usize| u16::from_le_bytes([data[o], data[o + 1]]);
    let sig_ix = u16_at(4);
    let pk_off = u16_at(6) as usize;
    let pk_ix = u16_at(8);
    let msg_off = u16_at(10) as usize;
    let msg_len = u16_at(12) as usize;
    let msg_ix = u16_at(14);
    if sig_ix != u16::MAX || pk_ix != u16::MAX || msg_ix != u16::MAX {
        return Err(OracleError::MissingSignature.into());
    }
    let pk = data.get(pk_off..pk_off + 32).ok_or(OracleError::Malformed)?;
    let msg = data.get(msg_off..msg_off.checked_add(msg_len).ok_or(OracleError::Malformed)?).ok_or(OracleError::Malformed)?;
    Ok((pk, msg))
}

fn verify(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let consumer = r.bytes32()?;
    let min_panel = r.u16()?;
    let min_agreed = r.u16()?;
    let n = r.u32()? as usize;
    let message = r.take(n)?;
    r.finish()?;
    let [cfg, ix_sysvar, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_state(cfg, program_id, config::DISC, config::LEN)?;
    let attester = read_bytes32(&cfg.try_borrow()?, config::ATTESTER);

    // 1. Предыдущая инструкция — Ed25519 program над нашим ключом и этими байтами.
    {
        let ixs = Instructions::try_from(&*ix_sysvar)?;
        let prev = ixs.get_instruction_relative(-1).map_err(|_| OracleError::MissingSignature)?;
        if prev.get_program_id() != &ED25519_PROGRAM {
            return Err(OracleError::MissingSignature.into());
        }
        let (pk, signed) = parse_ed25519(prev.get_instruction_data())?;
        if pk != attester {
            return Err(OracleError::WrongAttester.into());
        }
        if signed != message {
            return Err(OracleError::MessageMismatch.into());
        }
    }

    // 2. Домен и потребитель.
    if message.len() < 16 + 32 + tail::LEN || &message[..16] != DOMAIN {
        return Err(OracleError::WrongDomain.into());
    }
    if message[16..48] != consumer {
        return Err(OracleError::WrongConsumer.into());
    }

    // 3. Хвост фиксированной длины: размер панели, согласие, срок.
    let t = &message[message.len() - tail::LEN..];
    let panel_size = read_u16(t, tail::PANEL_SIZE);
    let agreed = read_u16(t, tail::AGREED);
    let expires_at = read_i64(t, tail::EXPIRES_AT);
    if now()? >= expires_at {
        return Err(OracleError::Expired.into());
    }
    if panel_size < min_panel {
        return Err(OracleError::PanelTooSmall.into());
    }
    if agreed < min_agreed {
        return Err(OracleError::NotEnoughAgreement.into());
    }
    Ok(())
}
