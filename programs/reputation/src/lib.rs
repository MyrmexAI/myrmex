//! `reputation` — реестр репутации (аналог реестра ERC-8004 ).
//!
//! Сервер (`writer`) пачкой записывает квитанции принятых работ: `ReceiptBatch` (PDA `["batch", seq]`)
//! с парами `(seat, recordHash)`, где `recordHash` — sha256 канонического JSON `work-record`/`review`,
//! а сам документ отдаётся по `GET /work-records/:hash.json`. Номера пачек идут подряд без пропусков.

use pinocchio::{AccountView, Address, ProgramResult, cpi::Seed, error::ProgramError, sysvars::{Sysvar, clock::Clock}};
use shank::{ShankInstruction, ShankType};
use swarm_program_utils::*;

solana_address::declare_id!("9gtUN67NoaVvNyM8zRVFefk7iyPdxpBs2n8YbASDo98L");

#[cfg(target_os = "solana")]
pinocchio::entrypoint!(process_instruction);

pub mod config {
    pub const DISC: u8 = 1;
    pub const SEED: &[u8] = b"config";
    pub const BUMP: usize = 2;
    pub const ADMIN: usize = 8;
    pub const WRITER: usize = 40;
    pub const NEXT_SEQ: usize = 72;
    pub const TOTAL_RECEIPTS: usize = 80;
    pub const LEN: usize = 88;
}

pub mod batch {
    pub const DISC: u8 = 2;
    pub const SEED: &[u8] = b"batch";
    pub const BUMP: usize = 2;
    pub const COUNT: usize = 4; // u16
    pub const SEQ: usize = 8;
    pub const SLOT: usize = 16;
    pub const CREATED_AT: usize = 24;
    pub const ENTRIES: usize = 32; // count × (seat 32 | record_hash 32)
    pub const ENTRY_LEN: usize = 64;
    pub const MAX_ENTRIES: usize = 48;
    pub const fn len(count: usize) -> usize {
        ENTRIES + count * ENTRY_LEN
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReputationError {
    Unauthorized = 0,
    BadBatchSize = 1,
}

impl From<ReputationError> for ProgramError {
    fn from(e: ReputationError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

/// Квитанция (Borsh: seat[32] | record_hash[32]).
#[derive(ShankType)]
pub struct Receipt {
    pub seat: [u8; 32],
    pub record_hash: [u8; 32],
}

#[derive(ShankInstruction)]
#[rustfmt::skip]
pub enum ReputationInstruction {
    #[account(0, writable, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    #[account(2, name = "writer")]
    #[account(3, name = "system_program")]
    Initialize,

    #[account(0, writable, signer, name = "writer")]
    #[account(1, writable, name = "config")]
    #[account(2, writable, name = "batch")]
    #[account(3, name = "system_program")]
    SubmitBatch { entries: Vec<Receipt> },

    #[account(0, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    SetAuthorities { admin: [u8; 32], writer: [u8; 32] },
}

pub mod ix {
    pub const INITIALIZE: u8 = 0;
    pub const SUBMIT_BATCH: u8 = 1;
    pub const SET_AUTHORITIES: u8 = 2;
}

pub fn process_instruction(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let (disc, rest) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    let mut r = Reader::new(rest);
    match *disc {
        ix::INITIALIZE => initialize(program_id, accounts, &r),
        ix::SUBMIT_BATCH => submit_batch(program_id, accounts, &mut r),
        ix::SET_AUTHORITIES => set_authorities(program_id, accounts, &mut r),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn initialize(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let [admin, cfg, writer, system_program, ..] = accounts else {
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
    write_bytes(&mut d, config::WRITER, writer.address().as_array());
    Ok(())
}

fn submit_batch(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let count = r.u32()? as usize;
    if count == 0 || count > batch::MAX_ENTRIES {
        return Err(ReputationError::BadBatchSize.into());
    }
    let entries = r.take(count * batch::ENTRY_LEN)?;
    r.finish()?;
    let [writer, cfg, batch_acc, system_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(writer)?;
    require_system_program(system_program)?;
    require_state(cfg, program_id, config::DISC, config::LEN)?;
    let seq = {
        let d = cfg.try_borrow()?;
        if read_addr(&d, config::WRITER) != *writer.address() {
            return Err(ReputationError::Unauthorized.into());
        }
        read_u64(&d, config::NEXT_SEQ)
    };
    let seq_b = seq.to_le_bytes();
    let (key, bump) = find_pda(&[batch::SEED, &seq_b], program_id);
    require_key(batch_acc, &key)?;
    let b = [bump];
    create_pda_account(writer, batch_acc, batch::len(count), program_id, &[Seed::from(batch::SEED), Seed::from(&seq_b), Seed::from(&b)])?;
    let clock = Clock::get()?;
    {
        let mut d = batch_acc.try_borrow_mut()?;
        d[0] = batch::DISC;
        d[1] = 1;
        d[batch::BUMP] = bump;
        write_u16(&mut d, batch::COUNT, count as u16);
        write_u64(&mut d, batch::SEQ, seq);
        write_u64(&mut d, batch::SLOT, clock.slot);
        write_i64(&mut d, batch::CREATED_AT, clock.unix_timestamp);
        write_bytes(&mut d, batch::ENTRIES, entries);
    }
    let mut d = cfg.try_borrow_mut()?;
    write_u64(&mut d, config::NEXT_SEQ, seq.checked_add(1).ok_or(ProgramError::ArithmeticOverflow)?);
    let total = read_u64(&d, config::TOTAL_RECEIPTS).checked_add(count as u64).ok_or(ProgramError::ArithmeticOverflow)?;
    write_u64(&mut d, config::TOTAL_RECEIPTS, total);
    Ok(())
}

fn set_authorities(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let admin_new = r.bytes32()?;
    let writer_new = r.bytes32()?;
    r.finish()?;
    let [admin, cfg, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_state(cfg, program_id, config::DISC, config::LEN)?;
    let mut d = cfg.try_borrow_mut()?;
    if read_addr(&d, config::ADMIN) != *admin.address() {
        return Err(ReputationError::Unauthorized.into());
    }
    write_bytes(&mut d, config::ADMIN, &admin_new);
    write_bytes(&mut d, config::WRITER, &writer_new);
    Ok(())
}
