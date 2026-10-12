//! `agent_registry` — регистрация агентов (аналог ERC-8004).
//!
//! У каждого места `seat_registry` может быть один агент: `Agent` (PDA `["agent", seat]`) с
//! последовательным `agent_id` и `agent_uri` — ссылкой на JSON агента (навыки, среды, версия
//! воркера), который отдаёт `GET /agents/by-token/:seatId.json`. Регистрирует и меняет URI
//! владелец места; когда место закрыто, запись может закрыть кто угодно, рента — владельцу.

use pinocchio::{AccountView, Address, ProgramResult, cpi::Seed, error::ProgramError};
use shank::ShankInstruction;
use swarm_program_utils::*;

solana_address::declare_id!("8utxdRTHjCC6QiyE8vHCvyQ5rCXjERAUvZkYCyP3dq1F");

#[cfg(target_os = "solana")]
pinocchio::entrypoint!(process_instruction);

/// Раскладка `Seat` из `seat_registry` (дублируется здесь, чтобы не тянуть его entrypoint).
pub mod seat_ref {
    use pinocchio::Address;
    pub const PROGRAM: Address = Address::from_str_const("HumVGc5FYBMXMGrPJHzZ9THegKcnTjAopYt5Za8jHBgB");
    pub const DISC: u8 = 2;
    pub const LEN: usize = 112;
    pub const STATUS: usize = 4;
    pub const OWNER: usize = 8;
    pub const ACTIVE: u8 = 0;
}

pub mod config {
    pub const DISC: u8 = 1;
    pub const SEED: &[u8] = b"config";
    pub const BUMP: usize = 2;
    pub const ADMIN: usize = 8;
    pub const NEXT_AGENT_ID: usize = 40;
    pub const LEN: usize = 48;
}

pub mod agent {
    pub const DISC: u8 = 2;
    pub const SEED: &[u8] = b"agent";
    pub const BUMP: usize = 2;
    pub const URI_LEN: usize = 4; // u16
    pub const AGENT_ID: usize = 8;
    pub const SEAT: usize = 16;
    pub const OWNER: usize = 48;
    pub const CREATED_AT: usize = 80;
    pub const UPDATED_AT: usize = 88;
    pub const URI: usize = 96;
    pub const MAX_URI: usize = 256;
    pub const LEN: usize = URI + MAX_URI;
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentError {
    NotSeatOwner = 0,
    SeatInactive = 1,
    BadUri = 2,
    SeatStillOpen = 3,
    WrongAccount = 4,
}

impl From<AgentError> for ProgramError {
    fn from(e: AgentError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

#[derive(ShankInstruction)]
#[rustfmt::skip]
pub enum AgentInstruction {
    #[account(0, writable, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    #[account(2, name = "system_program")]
    Initialize,

    #[account(0, writable, signer, name = "owner")]
    #[account(1, writable, name = "config")]
    #[account(2, name = "seat")]
    #[account(3, writable, name = "agent")]
    #[account(4, name = "system_program")]
    Register { uri: String },

    #[account(0, signer, name = "owner")]
    #[account(1, name = "seat")]
    #[account(2, writable, name = "agent")]
    UpdateUri { uri: String },

    #[account(0, writable, name = "agent")]
    #[account(1, name = "seat")]
    #[account(2, writable, name = "owner")]
    Close,
}

pub mod ix {
    pub const INITIALIZE: u8 = 0;
    pub const REGISTER: u8 = 1;
    pub const UPDATE_URI: u8 = 2;
    pub const CLOSE: u8 = 3;
}

pub fn process_instruction(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let (disc, rest) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    let mut r = Reader::new(rest);
    match *disc {
        ix::INITIALIZE => initialize(program_id, accounts, &r),
        ix::REGISTER => register(program_id, accounts, &mut r),
        ix::UPDATE_URI => update_uri(program_id, accounts, &mut r),
        ix::CLOSE => close(program_id, accounts, &r),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn read_uri<'a>(r: &mut Reader<'a>) -> Result<&'a [u8], ProgramError> {
    let n = r.u32()? as usize;
    let uri = r.take(n)?;
    r.finish()?;
    if n == 0 || n > agent::MAX_URI || core::str::from_utf8(uri).is_err() {
        return Err(AgentError::BadUri.into());
    }
    Ok(uri)
}

/// Место существует, принадлежит `owner` и активно.
fn check_seat(seat: &AccountView, owner: &Address) -> ProgramResult {
    require_state(seat, &seat_ref::PROGRAM, seat_ref::DISC, seat_ref::LEN)?;
    let d = seat.try_borrow()?;
    if read_addr(&d, seat_ref::OWNER) != *owner {
        return Err(AgentError::NotSeatOwner.into());
    }
    if d[seat_ref::STATUS] != seat_ref::ACTIVE {
        return Err(AgentError::SeatInactive.into());
    }
    Ok(())
}

fn write_uri(d: &mut [u8], uri: &[u8]) {
    d[agent::URI..agent::LEN].fill(0);
    write_bytes(d, agent::URI, uri);
    write_u16(d, agent::URI_LEN, uri.len() as u16);
}

fn initialize(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
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
    write_u64(&mut d, config::NEXT_AGENT_ID, 1);
    Ok(())
}

fn register(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let uri = read_uri(r)?;
    let [owner, cfg, seat, agent_acc, system_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(owner)?;
    require_system_program(system_program)?;
    require_state(cfg, program_id, config::DISC, config::LEN)?;
    check_seat(seat, owner.address())?;
    let (key, bump) = find_pda(&[agent::SEED, seat.address().as_array()], program_id);
    require_key(agent_acc, &key)?;
    if !agent_acc.is_data_empty() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let b = [bump];
    create_pda_account(owner, agent_acc, agent::LEN, program_id, &[Seed::from(agent::SEED), Seed::from(seat.address().as_array()), Seed::from(&b)])?;
    let id = {
        let mut d = cfg.try_borrow_mut()?;
        let id = read_u64(&d, config::NEXT_AGENT_ID);
        write_u64(&mut d, config::NEXT_AGENT_ID, id.checked_add(1).ok_or(ProgramError::ArithmeticOverflow)?);
        id
    };
    let now = now()?;
    let mut d = agent_acc.try_borrow_mut()?;
    d[0] = agent::DISC;
    d[1] = 1;
    d[agent::BUMP] = bump;
    write_u64(&mut d, agent::AGENT_ID, id);
    write_bytes(&mut d, agent::SEAT, seat.address().as_array());
    write_bytes(&mut d, agent::OWNER, owner.address().as_array());
    write_i64(&mut d, agent::CREATED_AT, now);
    write_i64(&mut d, agent::UPDATED_AT, now);
    write_uri(&mut d, uri);
    Ok(())
}

fn update_uri(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let uri = read_uri(r)?;
    let [owner, seat, agent_acc, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(owner)?;
    check_seat(seat, owner.address())?;
    require_state(agent_acc, program_id, agent::DISC, agent::LEN)?;
    let now = now()?;
    let mut d = agent_acc.try_borrow_mut()?;
    if read_addr(&d, agent::SEAT) != *seat.address() {
        return Err(AgentError::WrongAccount.into());
    }
    write_bytes(&mut d, agent::OWNER, owner.address().as_array());
    write_i64(&mut d, agent::UPDATED_AT, now);
    write_uri(&mut d, uri);
    Ok(())
}

fn close(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let [agent_acc, seat, owner, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_state(agent_acc, program_id, agent::DISC, agent::LEN)?;
    {
        let d = agent_acc.try_borrow()?;
        if read_addr(&d, agent::SEAT) != *seat.address() || read_addr(&d, agent::OWNER) != *owner.address() {
            return Err(AgentError::WrongAccount.into());
        }
    }
    // место должно быть закрыто (аккаунт удалён из seat_registry)
    if seat.owned_by(&seat_ref::PROGRAM) && !seat.is_data_empty() {
        return Err(AgentError::SeatStillOpen.into());
    }
    close_account(agent_acc, owner)
}
