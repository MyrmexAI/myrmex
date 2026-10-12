//! `seat_registry` — места в рое вместо NFT.
//!
//! Холдер блокирует `stake_amount` токенов и получает место `Seat` (PDA `["seat", owner, index]`,
//! `index < max_seats` — так лимит мест на кошелёк держится самой адресацией). К месту привязывается
//! ровно один ключ устройства. Выход: `request_unlock` → задержка → `withdraw`. Штраф: `propose_slash`
//! → таймлок (апелляция = `cancel_slash`) → `execute_slash`; пока штраф висит, вывод запрещён.
//! Смена конфига тоже через таймлок: `propose_config` → `apply_config`.

pub mod state;

use pinocchio::{AccountView, Address, ProgramResult, cpi::Seed, error::ProgramError};
use shank::ShankInstruction;
use state::{SEED_VAULT, config as c, seat as s, status};
use swarm_program_utils::*;

solana_address::declare_id!("HumVGc5FYBMXMGrPJHzZ9THegKcnTjAopYt5Za8jHBgB");

#[cfg(target_os = "solana")]
pinocchio::entrypoint!(process_instruction);

/// Ошибки программы (`ProgramError::Custom`).
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeatError {
    IndexOutOfRange = 0,
    WrongOwner = 1,
    DeviceAlreadyBound = 2,
    BadStatus = 3,
    UnlockNotReady = 4,
    SlashPending = 5,
    SlashTooLarge = 6,
    TimelockNotElapsed = 7,
    NoPendingChange = 8,
    WrongMint = 9,
    WrongVault = 10,
    WrongTreasury = 11,
    Unauthorized = 12,
    InvalidParams = 13,
    VaultNotEmpty = 14,
    /// Залог меньше минимального `stake_amount` из конфига.
    StakeTooLow = 15,
    /// Открыто уже `MAX_TOTAL_SEATS` мест — новое место откроется, когда кто-то выведет залог.
    SeatsFull = 16,
}

impl From<SeatError> for ProgramError {
    fn from(e: SeatError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

#[derive(ShankInstruction)]
#[rustfmt::skip]
pub enum SeatInstruction {
    #[account(0, writable, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    #[account(2, writable, name = "vault")]
    #[account(3, name = "mint")]
    #[account(4, name = "treasury")]
    #[account(5, name = "slash_authority")]
    #[account(6, name = "system_program")]
    #[account(7, name = "token_program")]
    Initialize { stake_amount: u64, unlock_delay: i64, timelock: i64, max_seats: u8, max_slash_bps: u16 },

    #[account(0, writable, signer, name = "owner")]
    #[account(1, writable, name = "config")]
    #[account(2, writable, name = "seat")]
    #[account(3, writable, name = "owner_token")]
    #[account(4, writable, name = "vault")]
    #[account(5, name = "mint")]
    #[account(6, name = "system_program")]
    #[account(7, name = "token_program")]
    /// `amount` — сколько заблокировать: не меньше `stake_amount` (минимум в конфиге). Сумму на $100 по рыночной
    /// цене считает сервер (DexScreener) в момент открытия; старый формат без `amount` блокирует ровно минимум.
    OpenSeat { index: u8, amount: u64 },

    #[account(0, signer, name = "owner")]
    #[account(1, writable, name = "seat")]
    BindDevice { device_key: [u8; 32] },

    #[account(0, signer, name = "owner")]
    #[account(1, writable, name = "seat")]
    RotateDevice { device_key: [u8; 32] },

    #[account(0, signer, name = "owner")]
    #[account(1, writable, name = "seat")]
    RequestUnlock,

    #[account(0, writable, signer, name = "owner")]
    #[account(1, writable, name = "config")]
    #[account(2, writable, name = "seat")]
    #[account(3, writable, name = "owner_token")]
    #[account(4, writable, name = "vault")]
    #[account(5, name = "mint")]
    #[account(6, name = "token_program")]
    Withdraw,

    #[account(0, signer, name = "slash_authority")]
    #[account(1, name = "config")]
    #[account(2, writable, name = "seat")]
    ProposeSlash { bps: u16 },

    #[account(0, name = "config")]
    #[account(1, writable, name = "seat")]
    #[account(2, writable, name = "vault")]
    #[account(3, writable, name = "treasury")]
    #[account(4, name = "mint")]
    #[account(5, name = "token_program")]
    ExecuteSlash,

    #[account(0, signer, name = "slash_authority")]
    #[account(1, name = "config")]
    #[account(2, writable, name = "seat")]
    CancelSlash,

    #[account(0, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    #[account(2, name = "treasury")]
    ProposeConfig { admin: [u8; 32], slash_authority: [u8; 32], stake_amount: u64, unlock_delay: i64, max_seats: u8 },

    #[account(0, writable, name = "config")]
    ApplyConfig,

    /// Смена утилити-токена после деплоя: только админ и только пока хранилище залогов пусто (ничьи средства не затронуты).
    /// Новое хранилище — PDA `["vault", new_mint]`; ожидающая смена конфига сбрасывается.
    #[account(0, writable, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    #[account(2, name = "old_vault")]
    #[account(3, writable, name = "new_vault")]
    #[account(4, name = "new_mint")]
    #[account(5, name = "new_treasury")]
    #[account(6, name = "system_program")]
    #[account(7, name = "token_program")]
    SetMint { stake_amount: u64 },

    /// Передача прав сразу (решение 2026-10-11): новый админ, штрафующий и казна штрафов — без таймлока.
    /// Залог, задержка вывода и лимит мест по-прежнему меняются только через propose/apply с таймлоком,
    /// а штраф исполняется только после своего таймлока. Ожидающее предложение сбрасывается.
    #[account(0, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    #[account(2, name = "treasury")]
    SetAuthorities { admin: [u8; 32], slash_authority: [u8; 32] },
}

pub mod ix {
    pub const INITIALIZE: u8 = 0;
    pub const OPEN_SEAT: u8 = 1;
    pub const BIND_DEVICE: u8 = 2;
    pub const ROTATE_DEVICE: u8 = 3;
    pub const REQUEST_UNLOCK: u8 = 4;
    pub const WITHDRAW: u8 = 5;
    pub const PROPOSE_SLASH: u8 = 6;
    pub const EXECUTE_SLASH: u8 = 7;
    pub const CANCEL_SLASH: u8 = 8;
    pub const PROPOSE_CONFIG: u8 = 9;
    pub const APPLY_CONFIG: u8 = 10;
    pub const SET_MINT: u8 = 11;
    pub const SET_AUTHORITIES: u8 = 12;
}

/// Верхняя граница штрафа, зашитая в программу.
pub const MAX_SLASH_BPS_CAP: u16 = 1_000;

pub fn process_instruction(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let (disc, rest) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    let mut r = Reader::new(rest);
    match *disc {
        ix::INITIALIZE => initialize(program_id, accounts, &mut r),
        ix::OPEN_SEAT => open_seat(program_id, accounts, &mut r),
        ix::BIND_DEVICE => set_device(program_id, accounts, &mut r, true),
        ix::ROTATE_DEVICE => set_device(program_id, accounts, &mut r, false),
        ix::REQUEST_UNLOCK => request_unlock(program_id, accounts, &r),
        ix::WITHDRAW => withdraw(program_id, accounts, &r),
        ix::PROPOSE_SLASH => propose_slash(program_id, accounts, &mut r),
        ix::EXECUTE_SLASH => execute_slash(program_id, accounts, &r),
        ix::CANCEL_SLASH => cancel_slash(program_id, accounts, &r),
        ix::PROPOSE_CONFIG => propose_config(program_id, accounts, &mut r),
        ix::APPLY_CONFIG => apply_config(program_id, accounts, &r),
        ix::SET_MINT => set_mint(program_id, accounts, &mut r),
        ix::SET_AUTHORITIES => set_authorities(program_id, accounts, &mut r),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

// ---------- помощники ----------

fn load_config(program_id: &Address, config: &AccountView) -> ProgramResult {
    require_state(config, program_id, c::DISC, c::LEN)
}

fn load_seat(program_id: &Address, seat: &AccountView, owner: &Address) -> ProgramResult {
    require_state(seat, program_id, s::DISC, s::LEN)?;
    let d = seat.try_borrow()?;
    if read_addr(&d, s::OWNER) != *owner {
        return Err(SeatError::WrongOwner.into());
    }
    Ok(())
}

fn check_vault_and_mint(config: &AccountView, vault: &AccountView, mint: &AccountView) -> Result<u8, ProgramError> {
    let d = config.try_borrow()?;
    if *vault.address() != read_addr(&d, c::VAULT) {
        return Err(SeatError::WrongVault.into());
    }
    if *mint.address() != read_addr(&d, c::MINT) {
        return Err(SeatError::WrongMint.into());
    }
    Ok(d[c::BUMP])
}

// ---------- инструкции ----------

fn initialize(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let stake_amount = r.u64()?;
    let unlock_delay = r.i64()?;
    let timelock = r.i64()?;
    let max_seats = r.u8()?;
    let max_slash_bps = r.u16()?;
    r.finish()?;
    if stake_amount == 0 || unlock_delay < 0 || timelock < 0 || max_seats == 0 || max_slash_bps > MAX_SLASH_BPS_CAP {
        return Err(SeatError::InvalidParams.into());
    }

    let [admin, config, vault, mint, treasury, slash_authority, system_program, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_system_program(system_program)?;
    require_token_program(token_program)?;
    mint_decimals(mint)?;
    let (t_mint, _, _) = token_account_info(treasury)?;
    if t_mint != *mint.address() {
        return Err(SeatError::WrongTreasury.into());
    }

    let (config_key, bump) = find_pda(&[c::SEED], program_id);
    require_key(config, &config_key)?;
    let (vault_key, vault_bump) = find_pda(&[SEED_VAULT], program_id);
    require_key(vault, &vault_key)?;
    if !config.is_data_empty() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }

    let bump_b = [bump];
    create_pda_account(admin, config, c::LEN, program_id, &[Seed::from(c::SEED), Seed::from(&bump_b)])?;
    let vb = [vault_bump];
    create_pda_account(
        admin,
        vault,
        TOKEN_ACCOUNT_LEN,
        token_program_of(mint)?,
        &[Seed::from(SEED_VAULT), Seed::from(&vb)],
    )?;
    init_token_account(vault, mint, &config_key)?;

    let mut d = config.try_borrow_mut()?;
    d[0] = c::DISC;
    d[1] = c::VERSION;
    d[c::BUMP] = bump;
    d[c::VAULT_BUMP] = vault_bump;
    d[c::MAX_SEATS] = max_seats;
    write_bytes(&mut d, c::ADMIN, admin.address().as_array());
    write_bytes(&mut d, c::MINT, mint.address().as_array());
    write_bytes(&mut d, c::VAULT, vault_key.as_array());
    write_bytes(&mut d, c::TREASURY, treasury.address().as_array());
    write_bytes(&mut d, c::SLASH_AUTHORITY, slash_authority.address().as_array());
    write_u64(&mut d, c::STAKE_AMOUNT, stake_amount);
    write_i64(&mut d, c::UNLOCK_DELAY, unlock_delay);
    write_i64(&mut d, c::TIMELOCK, timelock);
    write_u16(&mut d, c::MAX_SLASH_BPS, max_slash_bps);
    Ok(())
}

fn open_seat(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let index = r.u8()?;
    let rest = r.rest();
    let amount = match rest.len() {
        0 => None,
        8 => Some(u64::from_le_bytes(rest.try_into().map_err(|_| ProgramError::InvalidInstructionData)?)),
        _ => return Err(ProgramError::InvalidInstructionData),
    };
    let [owner, config, seat, owner_token, vault, mint, system_program, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(owner)?;
    require_system_program(system_program)?;
    require_token_program(token_program)?;
    load_config(program_id, config)?;
    check_vault_and_mint(config, vault, mint)?;

    let (stake, max_seats, config_bump) = {
        let d = config.try_borrow()?;
        (read_u64(&d, c::STAKE_AMOUNT), d[c::MAX_SEATS], d[c::BUMP])
    };
    let _ = config_bump;
    if index >= max_seats {
        return Err(SeatError::IndexOutOfRange.into());
    }
    let stake = match amount {
        None => stake,
        Some(a) if a >= stake => a,
        Some(_) => return Err(SeatError::StakeTooLow.into()),
    };

    let idx = [index];
    let (seat_key, bump) = find_pda(&[s::SEED, owner.address().as_array(), &idx], program_id);
    require_key(seat, &seat_key)?;
    if !seat.is_data_empty() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let bump_b = [bump];
    create_pda_account(
        owner,
        seat,
        s::LEN,
        program_id,
        &[Seed::from(s::SEED), Seed::from(owner.address().as_array()), Seed::from(&idx), Seed::from(&bump_b)],
    )?;

    let decimals = mint_decimals(mint)?;
    transfer_checked(owner_token, mint, vault, owner, stake, decimals, None)?;

    let now = now()?;
    {
        let mut d = seat.try_borrow_mut()?;
        d[0] = s::DISC;
        d[1] = s::VERSION;
        d[s::BUMP] = bump;
        d[s::INDEX] = index;
        d[s::STATUS] = status::ACTIVE;
        write_bytes(&mut d, s::OWNER, owner.address().as_array());
        write_u64(&mut d, s::STAKED, stake);
        write_i64(&mut d, s::CREATED_AT, now);
    }
    let mut d = config.try_borrow_mut()?;
    let open = read_u64(&d, c::SEATS_OPEN).checked_add(1).ok_or(ProgramError::ArithmeticOverflow)?;
    if open > c::MAX_TOTAL_SEATS {
        return Err(SeatError::SeatsFull.into());
    }
    write_u64(&mut d, c::SEATS_OPEN, open);
    Ok(())
}

fn set_device(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader, bind_only: bool) -> ProgramResult {
    let device_key = r.bytes32()?;
    r.finish()?;
    let [owner, seat, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(owner)?;
    load_seat(program_id, seat, owner.address())?;
    let mut d = seat.try_borrow_mut()?;
    if d[s::STATUS] != status::ACTIVE {
        return Err(SeatError::BadStatus.into());
    }
    if bind_only && read_bytes32(&d, s::DEVICE_KEY) != [0u8; 32] {
        return Err(SeatError::DeviceAlreadyBound.into());
    }
    write_bytes(&mut d, s::DEVICE_KEY, &device_key);
    Ok(())
}

fn request_unlock(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let [owner, seat, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(owner)?;
    load_seat(program_id, seat, owner.address())?;
    let now = now()?;
    let mut d = seat.try_borrow_mut()?;
    if d[s::STATUS] == status::UNLOCKING {
        return Err(SeatError::BadStatus.into());
    }
    d[s::STATUS] = status::UNLOCKING;
    write_i64(&mut d, s::UNLOCK_REQUESTED_AT, now);
    // устройство отвязывается: на время задержки место не получает работу
    write_bytes(&mut d, s::DEVICE_KEY, &[0u8; 32]);
    Ok(())
}

fn withdraw(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let [owner, config, seat, owner_token, vault, mint, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(owner)?;
    require_token_program(token_program)?;
    load_config(program_id, config)?;
    let config_bump = check_vault_and_mint(config, vault, mint)?;
    load_seat(program_id, seat, owner.address())?;
    let unlock_delay = read_i64(&config.try_borrow()?, c::UNLOCK_DELAY);

    let staked = {
        let d = seat.try_borrow()?;
        if d[s::STATUS] != status::UNLOCKING {
            return Err(SeatError::BadStatus.into());
        }
        if read_u16(&d, s::PENDING_SLASH_BPS) != 0 {
            return Err(SeatError::SlashPending.into());
        }
        let ready_at = read_i64(&d, s::UNLOCK_REQUESTED_AT)
            .checked_add(unlock_delay)
            .ok_or(ProgramError::ArithmeticOverflow)?;
        if now()? < ready_at {
            return Err(SeatError::UnlockNotReady.into());
        }
        read_u64(&d, s::STAKED)
    };

    let (o_mint, _, _) = token_account_info(owner_token)?;
    if o_mint != *mint.address() {
        return Err(SeatError::WrongMint.into());
    }
    if staked > 0 {
        let decimals = mint_decimals(mint)?;
        let cb = [config_bump];
        transfer_checked(vault, mint, owner_token, config, staked, decimals, Some(&[Seed::from(c::SEED), Seed::from(&cb)]))?;
    }
    {
        let mut d = config.try_borrow_mut()?;
        let open = read_u64(&d, c::SEATS_OPEN).saturating_sub(1);
        write_u64(&mut d, c::SEATS_OPEN, open);
    }
    close_account(seat, owner)
}

fn propose_slash(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let bps = r.u16()?;
    r.finish()?;
    let [authority, config, seat, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(authority)?;
    load_config(program_id, config)?;
    require_state(seat, program_id, s::DISC, s::LEN)?;
    let (max_bps, timelock) = {
        let d = config.try_borrow()?;
        if read_addr(&d, c::SLASH_AUTHORITY) != *authority.address() {
            return Err(SeatError::Unauthorized.into());
        }
        (read_u16(&d, c::MAX_SLASH_BPS), read_i64(&d, c::TIMELOCK))
    };
    if bps == 0 || bps > max_bps {
        return Err(SeatError::SlashTooLarge.into());
    }
    let at = now()?.checked_add(timelock).ok_or(ProgramError::ArithmeticOverflow)?;
    let mut d = seat.try_borrow_mut()?;
    if read_u16(&d, s::PENDING_SLASH_BPS) != 0 {
        return Err(SeatError::SlashPending.into());
    }
    write_u16(&mut d, s::PENDING_SLASH_BPS, bps);
    write_i64(&mut d, s::SLASH_EXECUTABLE_AT, at);
    Ok(())
}

fn execute_slash(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let [config, seat, vault, treasury, mint, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_token_program(token_program)?;
    load_config(program_id, config)?;
    let config_bump = check_vault_and_mint(config, vault, mint)?;
    if *treasury.address() != read_addr(&config.try_borrow()?, c::TREASURY) {
        return Err(SeatError::WrongTreasury.into());
    }
    require_state(seat, program_id, s::DISC, s::LEN)?;

    let amount = {
        let mut d = seat.try_borrow_mut()?;
        let bps = read_u16(&d, s::PENDING_SLASH_BPS);
        if bps == 0 {
            return Err(SeatError::NoPendingChange.into());
        }
        if now()? < read_i64(&d, s::SLASH_EXECUTABLE_AT) {
            return Err(SeatError::TimelockNotElapsed.into());
        }
        let staked = read_u64(&d, s::STAKED);
        let amount = bps_of(staked, bps)?;
        write_u64(&mut d, s::STAKED, staked.checked_sub(amount).ok_or(ProgramError::ArithmeticOverflow)?);
        write_u16(&mut d, s::PENDING_SLASH_BPS, 0);
        write_i64(&mut d, s::SLASH_EXECUTABLE_AT, 0);
        if d[s::STATUS] == status::ACTIVE {
            d[s::STATUS] = status::SLASHED;
            write_bytes(&mut d, s::DEVICE_KEY, &[0u8; 32]);
        }
        amount
    };
    if amount > 0 {
        let decimals = mint_decimals(mint)?;
        let cb = [config_bump];
        transfer_checked(vault, mint, treasury, config, amount, decimals, Some(&[Seed::from(c::SEED), Seed::from(&cb)]))?;
    }
    Ok(())
}

fn cancel_slash(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let [authority, config, seat, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(authority)?;
    load_config(program_id, config)?;
    if read_addr(&config.try_borrow()?, c::SLASH_AUTHORITY) != *authority.address() {
        return Err(SeatError::Unauthorized.into());
    }
    require_state(seat, program_id, s::DISC, s::LEN)?;
    let mut d = seat.try_borrow_mut()?;
    if read_u16(&d, s::PENDING_SLASH_BPS) == 0 {
        return Err(SeatError::NoPendingChange.into());
    }
    write_u16(&mut d, s::PENDING_SLASH_BPS, 0);
    write_i64(&mut d, s::SLASH_EXECUTABLE_AT, 0);
    Ok(())
}

fn propose_config(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let new_admin = r.bytes32()?;
    let new_slash_authority = r.bytes32()?;
    let stake_amount = r.u64()?;
    let unlock_delay = r.i64()?;
    let max_seats = r.u8()?;
    r.finish()?;
    if stake_amount == 0 || unlock_delay < 0 || max_seats == 0 {
        return Err(SeatError::InvalidParams.into());
    }
    let [admin, config, treasury, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    load_config(program_id, config)?;
    let (t_mint, _, _) = token_account_info(treasury)?;
    let mut d = config.try_borrow_mut()?;
    if read_addr(&d, c::ADMIN) != *admin.address() {
        return Err(SeatError::Unauthorized.into());
    }
    if t_mint != read_addr(&d, c::MINT) {
        return Err(SeatError::WrongTreasury.into());
    }
    let at = now()?.checked_add(read_i64(&d, c::TIMELOCK)).ok_or(ProgramError::ArithmeticOverflow)?;
    d[c::HAS_PENDING] = 1;
    write_bytes(&mut d, c::P_ADMIN, &new_admin);
    write_bytes(&mut d, c::P_SLASH_AUTHORITY, &new_slash_authority);
    write_bytes(&mut d, c::P_TREASURY, treasury.address().as_array());
    write_u64(&mut d, c::P_STAKE_AMOUNT, stake_amount);
    write_i64(&mut d, c::P_UNLOCK_DELAY, unlock_delay);
    d[c::P_MAX_SEATS] = max_seats;
    write_i64(&mut d, c::P_EFFECTIVE_AT, at);
    Ok(())
}

fn set_authorities(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let new_admin = r.bytes32()?;
    let new_slash_authority = r.bytes32()?;
    r.finish()?;
    let [admin, config, treasury, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    load_config(program_id, config)?;
    let (t_mint, _, _) = token_account_info(treasury)?;
    let mut d = config.try_borrow_mut()?;
    if read_addr(&d, c::ADMIN) != *admin.address() {
        return Err(SeatError::Unauthorized.into());
    }
    if t_mint != read_addr(&d, c::MINT) {
        return Err(SeatError::WrongTreasury.into());
    }
    write_bytes(&mut d, c::ADMIN, &new_admin);
    write_bytes(&mut d, c::SLASH_AUTHORITY, &new_slash_authority);
    write_bytes(&mut d, c::TREASURY, treasury.address().as_array());
    // предложение старого админа больше не действует
    d[c::HAS_PENDING] = 0;
    Ok(())
}

fn apply_config(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let [config, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    load_config(program_id, config)?;
    let now = now()?;
    let mut d = config.try_borrow_mut()?;
    if d[c::HAS_PENDING] == 0 {
        return Err(SeatError::NoPendingChange.into());
    }
    if now < read_i64(&d, c::P_EFFECTIVE_AT) {
        return Err(SeatError::TimelockNotElapsed.into());
    }
    d.copy_within(c::P_ADMIN..c::P_ADMIN + 32, c::ADMIN);
    d.copy_within(c::P_SLASH_AUTHORITY..c::P_SLASH_AUTHORITY + 32, c::SLASH_AUTHORITY);
    d.copy_within(c::P_TREASURY..c::P_TREASURY + 32, c::TREASURY);
    d.copy_within(c::P_STAKE_AMOUNT..c::P_STAKE_AMOUNT + 8, c::STAKE_AMOUNT);
    d.copy_within(c::P_UNLOCK_DELAY..c::P_UNLOCK_DELAY + 8, c::UNLOCK_DELAY);
    d[c::MAX_SEATS] = d[c::P_MAX_SEATS];
    d[c::HAS_PENDING] = 0;
    d[c::P_ADMIN..c::LEN].fill(0);
    Ok(())
}

fn set_mint(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let stake_amount = r.u64()?;
    r.finish()?;
    if stake_amount == 0 {
        return Err(SeatError::InvalidParams.into());
    }
    let [admin, config, old_vault, new_vault, new_mint, new_treasury, system_program, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_system_program(system_program)?;
    require_token_program(token_program)?;
    load_config(program_id, config)?;
    {
        let d = config.try_borrow()?;
        if read_addr(&d, c::ADMIN) != *admin.address() {
            return Err(SeatError::Unauthorized.into());
        }
        if read_addr(&d, c::VAULT) != *old_vault.address() {
            return Err(SeatError::WrongVault.into());
        }
        if read_addr(&d, c::MINT) == *new_mint.address() {
            return Err(SeatError::InvalidParams.into());
        }
    }
    // в старом хранилище не должно быть ни одного залога
    let (_, _, held) = token_account_info(old_vault)?;
    if held != 0 {
        return Err(SeatError::VaultNotEmpty.into());
    }
    mint_decimals(new_mint)?;
    let (t_mint, _, _) = token_account_info(new_treasury)?;
    if t_mint != *new_mint.address() {
        return Err(SeatError::WrongTreasury.into());
    }
    let (vault_key, vault_bump) = find_pda(&[SEED_VAULT, new_mint.address().as_ref()], program_id);
    require_key(new_vault, &vault_key)?;
    if !new_vault.is_data_empty() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let vb = [vault_bump];
    create_pda_account(
        admin,
        new_vault,
        TOKEN_ACCOUNT_LEN,
        token_program_of(new_mint)?,
        &[Seed::from(SEED_VAULT), Seed::from(new_mint.address().as_ref()), Seed::from(&vb)],
    )?;
    init_token_account(new_vault, new_mint, config.address())?;

    let mut d = config.try_borrow_mut()?;
    d[c::VAULT_BUMP] = vault_bump;
    d[c::HAS_PENDING] = 0;
    write_bytes(&mut d, c::MINT, new_mint.address().as_array());
    write_bytes(&mut d, c::VAULT, vault_key.as_array());
    write_bytes(&mut d, c::TREASURY, new_treasury.address().as_array());
    write_u64(&mut d, c::STAKE_AMOUNT, stake_amount);
    Ok(())
}
