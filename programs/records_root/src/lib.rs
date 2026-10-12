//! `records_root` — суточные корни деревьев Меркла записей.
//!
//! Раз в сутки (UTC) сервер (`writer`) публикует корень дерева над `(jobId, documentHash)` одной
//! транзакцией: аккаунт `DailyRoot` (PDA `["day", day]`), неизменяемый после публикации.
//! `verify` проверяет доказательство листа `sha256(0x00 || jobId || documentHash)` — его можно
//! вызвать напрямую или через CPI из программы-потребителя.

use pinocchio::{AccountView, Address, ProgramResult, cpi::Seed, error::ProgramError};
use shank::ShankInstruction;
use swarm_program_utils::*;

solana_address::declare_id!("EH83WpiqWU1FKFpcVJ4tdeUrRTRafZHpPxtdMAwouSHU");

#[cfg(target_os = "solana")]
pinocchio::entrypoint!(process_instruction);

pub mod config {
    pub const DISC: u8 = 1;
    pub const SEED: &[u8] = b"config";
    pub const BUMP: usize = 2;
    pub const ADMIN: usize = 8;
    pub const WRITER: usize = 40;
    pub const DAYS: usize = 72; // u64, сколько корней опубликовано
    pub const LAST_DAY: usize = 80; // u32
    pub const LEN: usize = 88;
}

pub mod day {
    pub const DISC: u8 = 2;
    pub const SEED: &[u8] = b"day";
    pub const BUMP: usize = 2;
    pub const DAY: usize = 4; // u32: дней с 1970-01-01 UTC
    pub const ROOT: usize = 8;
    pub const COUNT: usize = 40; // u32: листьев в дереве
    pub const PUBLISHED_AT: usize = 48;
    pub const LEN: usize = 56;
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootError {
    Unauthorized = 0,
    FutureDay = 1,
    InvalidProof = 2,
    EmptyTree = 3,
}

impl From<RootError> for ProgramError {
    fn from(e: RootError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

#[derive(ShankInstruction)]
#[rustfmt::skip]
pub enum RecordsRootInstruction {
    #[account(0, writable, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    #[account(2, name = "writer")]
    #[account(3, name = "system_program")]
    Initialize,

    #[account(0, writable, signer, name = "writer")]
    #[account(1, writable, name = "config")]
    #[account(2, writable, name = "daily_root")]
    #[account(3, name = "system_program")]
    Publish { day: u32, root: [u8; 32], count: u32 },

    #[account(0, name = "daily_root")]
    Verify { job_id: [u8; 16], document_hash: [u8; 32], proof: Vec<[u8; 32]> },

    #[account(0, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    SetAuthorities { admin: [u8; 32], writer: [u8; 32] },
}

pub mod ix {
    pub const INITIALIZE: u8 = 0;
    pub const PUBLISH: u8 = 1;
    pub const VERIFY: u8 = 2;
    pub const SET_AUTHORITIES: u8 = 3;
}

pub fn process_instruction(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let (disc, rest) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    let mut r = Reader::new(rest);
    match *disc {
        ix::INITIALIZE => initialize(program_id, accounts, &r),
        ix::PUBLISH => publish(program_id, accounts, &mut r),
        ix::VERIFY => verify(program_id, accounts, &mut r),
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

fn publish(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let day_n = r.u32()?;
    let root = r.bytes32()?;
    let count = r.u32()?;
    r.finish()?;
    if count == 0 {
        return Err(RootError::EmptyTree.into());
    }
    let [writer, cfg, day_acc, system_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(writer)?;
    require_system_program(system_program)?;
    require_state(cfg, program_id, config::DISC, config::LEN)?;
    if read_addr(&cfg.try_borrow()?, config::WRITER) != *writer.address() {
        return Err(RootError::Unauthorized.into());
    }
    let now = now()?;
    // корень публикуется только за закрытый день
    if (day_n as i64 + 1) * 86_400 > now {
        return Err(RootError::FutureDay.into());
    }
    let day_b = day_n.to_le_bytes();
    let (key, bump) = find_pda(&[day::SEED, &day_b], program_id);
    require_key(day_acc, &key)?;
    if !day_acc.is_data_empty() {
        // день закрыт навсегда: корень не переписывается
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let b = [bump];
    create_pda_account(writer, day_acc, day::LEN, program_id, &[Seed::from(day::SEED), Seed::from(&day_b), Seed::from(&b)])?;
    {
        let mut d = day_acc.try_borrow_mut()?;
        d[0] = day::DISC;
        d[1] = 1;
        d[day::BUMP] = bump;
        d[day::DAY..day::DAY + 4].copy_from_slice(&day_b);
        write_bytes(&mut d, day::ROOT, &root);
        d[day::COUNT..day::COUNT + 4].copy_from_slice(&count.to_le_bytes());
        write_i64(&mut d, day::PUBLISHED_AT, now);
    }
    let mut d = cfg.try_borrow_mut()?;
    let days = read_u64(&d, config::DAYS).checked_add(1).ok_or(ProgramError::ArithmeticOverflow)?;
    write_u64(&mut d, config::DAYS, days);
    d[config::LAST_DAY..config::LAST_DAY + 4].copy_from_slice(&day_b);
    Ok(())
}

fn verify(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let job_id: [u8; 16] = r.take(16)?.try_into().unwrap();
    let doc = r.bytes32()?;
    let n = r.u32()? as usize;
    let proof = r.take(n.checked_mul(32).ok_or(ProgramError::InvalidInstructionData)?)?;
    r.finish()?;
    let [day_acc, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_state(day_acc, program_id, day::DISC, day::LEN)?;
    let root = read_bytes32(&day_acc.try_borrow()?, day::ROOT);
    let leaf = sha256(&[&[LEAF_RECORD], &job_id, &doc]);
    if !verify_proof(leaf, proof, &root)? {
        return Err(RootError::InvalidProof.into());
    }
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
        return Err(RootError::Unauthorized.into());
    }
    write_bytes(&mut d, config::ADMIN, &admin_new);
    write_bytes(&mut d, config::WRITER, &writer_new);
    Ok(())
}
