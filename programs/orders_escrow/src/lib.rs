//! `orders_escrow` — эскроу оплаты заказов.
//!
//! Поток: сервер выдал котировку → заказчик подписывает `pay` (перевод токена в эскроу с `order_id`
//! котировки), сервер (`operator`) со-подписывает как fee payer и плательщик ренты — так в эскроу
//! попадают только заказы, которые сервер сам котировал. После приёмки работы `release` делит сумму:
//! `burn_bps` сжигается, `treasury_bps` уходит в казну, остаток — в хранилище выплат (`payout`,
//! из него `distributor` раздаёт исполнителям и рецензентам). При отказе — `refund` заказчику;
//! если сервер молчит дольше `deadline + refund_grace`, вернуть деньги может любой.

use pinocchio::{AccountView, Address, ProgramResult, cpi::Seed, error::ProgramError};
use shank::ShankInstruction;
use swarm_program_utils::*;

solana_address::declare_id!("2kXF8CFBtmZL4d6QQae93EBr5NVzWeWAV93EKVMsF5X3");

#[cfg(target_os = "solana")]
pinocchio::entrypoint!(process_instruction);

pub mod config {
    pub const DISC: u8 = 1;
    pub const SEED: &[u8] = b"config";
    pub const BUMP: usize = 2;
    pub const VAULT_BUMP: usize = 3;
    pub const ADMIN: usize = 8;
    pub const OPERATOR: usize = 40;
    pub const MINT: usize = 72;
    pub const VAULT: usize = 104;
    pub const PAYOUT: usize = 136;
    pub const TREASURY: usize = 168;
    pub const BURN_BPS: usize = 200;
    pub const TREASURY_BPS: usize = 202;
    pub const REFUND_GRACE: usize = 208;
    pub const TOTAL_PAID: usize = 216;
    pub const TOTAL_BURNED: usize = 224;
    pub const LEN: usize = 232;
}

pub mod order {
    pub const DISC: u8 = 2;
    pub const SEED: &[u8] = b"order";
    pub const BUMP: usize = 2;
    pub const STATUS: usize = 3;
    pub const ORDER_ID: usize = 8; // 16 байт UUID
    pub const PAYER: usize = 24;
    pub const PAYER_TOKEN: usize = 56; // куда возвращать при refund
    pub const RENT_PAYER: usize = 88;
    pub const AMOUNT: usize = 120;
    pub const CREATED_AT: usize = 128;
    pub const DEADLINE: usize = 136;
    pub const LEN: usize = 144;
}

pub const SEED_VAULT: &[u8] = b"vault";

pub mod status {
    pub const FUNDED: u8 = 1;
    pub const RELEASED: u8 = 2;
    pub const REFUNDED: u8 = 3;
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscrowError {
    Unauthorized = 0,
    BadStatus = 1,
    InvalidParams = 2,
    WrongAccount = 3,
    RefundNotAllowed = 4,
    VaultNotEmpty = 5,
}

impl From<EscrowError> for ProgramError {
    fn from(e: EscrowError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

#[derive(ShankInstruction)]
#[rustfmt::skip]
pub enum EscrowInstruction {
    #[account(0, writable, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    #[account(2, writable, name = "vault")]
    #[account(3, name = "mint")]
    #[account(4, name = "operator")]
    #[account(5, name = "payout")]
    #[account(6, name = "treasury")]
    #[account(7, name = "system_program")]
    #[account(8, name = "token_program")]
    Initialize { burn_bps: u16, treasury_bps: u16, refund_grace: i64 },

    #[account(0, signer, name = "payer")]
    #[account(1, writable, signer, name = "operator")]
    #[account(2, name = "config")]
    #[account(3, writable, name = "order")]
    #[account(4, writable, name = "payer_token")]
    #[account(5, writable, name = "vault")]
    #[account(6, name = "mint")]
    #[account(7, name = "system_program")]
    #[account(8, name = "token_program")]
    Pay { order_id: [u8; 16], amount: u64, deadline: i64 },

    #[account(0, signer, name = "operator")]
    #[account(1, writable, name = "config")]
    #[account(2, writable, name = "order")]
    #[account(3, writable, name = "vault")]
    #[account(4, writable, name = "payout")]
    #[account(5, writable, name = "treasury")]
    #[account(6, writable, name = "mint")]
    #[account(7, name = "token_program")]
    Release,

    #[account(0, signer, name = "caller")]
    #[account(1, name = "config")]
    #[account(2, writable, name = "order")]
    #[account(3, writable, name = "vault")]
    #[account(4, writable, name = "payer_token")]
    #[account(5, name = "mint")]
    #[account(6, name = "token_program")]
    Refund,

    #[account(0, writable, name = "order")]
    #[account(1, writable, name = "rent_payer")]
    CloseOrder,

    #[account(0, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    #[account(2, name = "payout")]
    #[account(3, name = "treasury")]
    SetConfig { admin: [u8; 32], operator: [u8; 32], burn_bps: u16, treasury_bps: u16, refund_grace: i64 },

    /// Смена токена оплаты после деплоя: только админ и только при пустом эскроу (нет открытых заказов со средствами).
    /// Новое хранилище — PDA `["vault", new_mint]`; получатели выплат и казна — токен-аккаунты нового токена.
    #[account(0, writable, signer, name = "admin")]
    #[account(1, writable, name = "config")]
    #[account(2, name = "old_vault")]
    #[account(3, writable, name = "new_vault")]
    #[account(4, name = "new_mint")]
    #[account(5, name = "payout")]
    #[account(6, name = "treasury")]
    #[account(7, name = "system_program")]
    #[account(8, name = "token_program")]
    SetMint,
}

pub mod ix {
    pub const INITIALIZE: u8 = 0;
    pub const PAY: u8 = 1;
    pub const RELEASE: u8 = 2;
    pub const REFUND: u8 = 3;
    pub const CLOSE_ORDER: u8 = 4;
    pub const SET_CONFIG: u8 = 5;
    pub const SET_MINT: u8 = 6;
}

pub fn process_instruction(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let (disc, rest) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    let mut r = Reader::new(rest);
    match *disc {
        ix::INITIALIZE => initialize(program_id, accounts, &mut r),
        ix::PAY => pay(program_id, accounts, &mut r),
        ix::RELEASE => release(program_id, accounts, &r),
        ix::REFUND => refund(program_id, accounts, &r),
        ix::CLOSE_ORDER => close_order(program_id, accounts, &r),
        ix::SET_CONFIG => set_config(program_id, accounts, &mut r),
        ix::SET_MINT => set_mint(program_id, accounts, &r),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn check_split(burn_bps: u16, treasury_bps: u16) -> ProgramResult {
    if (burn_bps as u32) + (treasury_bps as u32) > 10_000 {
        return Err(EscrowError::InvalidParams.into());
    }
    Ok(())
}

fn check_token_of(account: &AccountView, mint: &Address) -> ProgramResult {
    let (m, _, _) = token_account_info(account)?;
    if m != *mint {
        return Err(EscrowError::WrongAccount.into());
    }
    Ok(())
}

fn initialize(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let burn_bps = r.u16()?;
    let treasury_bps = r.u16()?;
    let refund_grace = r.i64()?;
    r.finish()?;
    check_split(burn_bps, treasury_bps)?;
    if refund_grace < 0 {
        return Err(EscrowError::InvalidParams.into());
    }
    let [admin, cfg, vault, mint, operator, payout, treasury, system_program, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_system_program(system_program)?;
    require_token_program(token_program)?;
    mint_decimals(mint)?;
    check_token_of(payout, mint.address())?;
    check_token_of(treasury, mint.address())?;
    let (cfg_key, bump) = find_pda(&[config::SEED], program_id);
    require_key(cfg, &cfg_key)?;
    let (vault_key, vault_bump) = find_pda(&[SEED_VAULT], program_id);
    require_key(vault, &vault_key)?;
    if !cfg.is_data_empty() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let b = [bump];
    create_pda_account(admin, cfg, config::LEN, program_id, &[Seed::from(config::SEED), Seed::from(&b)])?;
    let vb = [vault_bump];
    create_vault(admin, vault, mint, &cfg_key, &[Seed::from(SEED_VAULT), Seed::from(&vb)])?;

    let mut d = cfg.try_borrow_mut()?;
    d[0] = config::DISC;
    d[1] = 1;
    d[config::BUMP] = bump;
    d[config::VAULT_BUMP] = vault_bump;
    write_bytes(&mut d, config::ADMIN, admin.address().as_array());
    write_bytes(&mut d, config::OPERATOR, operator.address().as_array());
    write_bytes(&mut d, config::MINT, mint.address().as_array());
    write_bytes(&mut d, config::VAULT, vault_key.as_array());
    write_bytes(&mut d, config::PAYOUT, payout.address().as_array());
    write_bytes(&mut d, config::TREASURY, treasury.address().as_array());
    write_u16(&mut d, config::BURN_BPS, burn_bps);
    write_u16(&mut d, config::TREASURY_BPS, treasury_bps);
    write_i64(&mut d, config::REFUND_GRACE, refund_grace);
    Ok(())
}

fn pay(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let order_id: [u8; 16] = r.take(16)?.try_into().unwrap();
    let amount = r.u64()?;
    let deadline = r.i64()?;
    r.finish()?;
    if amount == 0 {
        return Err(EscrowError::InvalidParams.into());
    }
    let [payer, operator, cfg, ord, payer_token, vault, mint, system_program, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(payer)?;
    require_signer(operator)?;
    require_system_program(system_program)?;
    require_token_program(token_program)?;
    require_state(cfg, program_id, config::DISC, config::LEN)?;
    {
        let d = cfg.try_borrow()?;
        if read_addr(&d, config::OPERATOR) != *operator.address() {
            return Err(EscrowError::Unauthorized.into());
        }
        if read_addr(&d, config::VAULT) != *vault.address() || read_addr(&d, config::MINT) != *mint.address() {
            return Err(EscrowError::WrongAccount.into());
        }
    }
    let now = now()?;
    if deadline <= now {
        return Err(EscrowError::InvalidParams.into());
    }
    let (ord_key, bump) = find_pda(&[order::SEED, &order_id], program_id);
    require_key(ord, &ord_key)?;
    if !ord.is_data_empty() {
        // повтор той же оплаты никогда не списывает дважды
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let b = [bump];
    create_pda_account(operator, ord, order::LEN, program_id, &[Seed::from(order::SEED), Seed::from(&order_id), Seed::from(&b)])?;
    let decimals = mint_decimals(mint)?;
    transfer_checked(payer_token, mint, vault, payer, amount, decimals, None)?;

    let mut d = ord.try_borrow_mut()?;
    d[0] = order::DISC;
    d[1] = 1;
    d[order::BUMP] = bump;
    d[order::STATUS] = status::FUNDED;
    write_bytes(&mut d, order::ORDER_ID, &order_id);
    write_bytes(&mut d, order::PAYER, payer.address().as_array());
    write_bytes(&mut d, order::PAYER_TOKEN, payer_token.address().as_array());
    write_bytes(&mut d, order::RENT_PAYER, operator.address().as_array());
    write_u64(&mut d, order::AMOUNT, amount);
    write_i64(&mut d, order::CREATED_AT, now);
    write_i64(&mut d, order::DEADLINE, deadline);
    Ok(())
}

fn release(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let [operator, cfg, ord, vault, payout, treasury, mint, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(operator)?;
    require_token_program(token_program)?;
    require_state(cfg, program_id, config::DISC, config::LEN)?;
    require_state(ord, program_id, order::DISC, order::LEN)?;
    let (bump, burn_bps, treasury_bps) = {
        let d = cfg.try_borrow()?;
        if read_addr(&d, config::OPERATOR) != *operator.address() {
            return Err(EscrowError::Unauthorized.into());
        }
        if read_addr(&d, config::VAULT) != *vault.address()
            || read_addr(&d, config::MINT) != *mint.address()
            || read_addr(&d, config::PAYOUT) != *payout.address()
            || read_addr(&d, config::TREASURY) != *treasury.address()
        {
            return Err(EscrowError::WrongAccount.into());
        }
        (d[config::BUMP], read_u16(&d, config::BURN_BPS), read_u16(&d, config::TREASURY_BPS))
    };
    let amount = {
        let mut d = ord.try_borrow_mut()?;
        if d[order::STATUS] != status::FUNDED {
            return Err(EscrowError::BadStatus.into());
        }
        d[order::STATUS] = status::RELEASED;
        read_u64(&d, order::AMOUNT)
    };
    let burn = bps_of(amount, burn_bps)?;
    let to_treasury = bps_of(amount, treasury_bps)?;
    let to_payout = amount
        .checked_sub(burn)
        .and_then(|v| v.checked_sub(to_treasury))
        .ok_or(ProgramError::ArithmeticOverflow)?;

    let decimals = mint_decimals(mint)?;
    let b = [bump];
    let seeds = [Seed::from(config::SEED), Seed::from(&b)];
    if burn > 0 {
        burn_checked(vault, mint, cfg, burn, decimals, &seeds)?;
    }
    if to_treasury > 0 {
        transfer_checked(vault, mint, treasury, cfg, to_treasury, decimals, Some(&seeds))?;
    }
    if to_payout > 0 {
        transfer_checked(vault, mint, payout, cfg, to_payout, decimals, Some(&seeds))?;
    }
    let mut d = cfg.try_borrow_mut()?;
    let paid = read_u64(&d, config::TOTAL_PAID).checked_add(amount).ok_or(ProgramError::ArithmeticOverflow)?;
    let burned = read_u64(&d, config::TOTAL_BURNED).checked_add(burn).ok_or(ProgramError::ArithmeticOverflow)?;
    write_u64(&mut d, config::TOTAL_PAID, paid);
    write_u64(&mut d, config::TOTAL_BURNED, burned);
    Ok(())
}

fn refund(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let [caller, cfg, ord, vault, payer_token, mint, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(caller)?;
    require_token_program(token_program)?;
    require_state(cfg, program_id, config::DISC, config::LEN)?;
    require_state(ord, program_id, order::DISC, order::LEN)?;
    let (bump, operator, grace) = {
        let d = cfg.try_borrow()?;
        if read_addr(&d, config::VAULT) != *vault.address() || read_addr(&d, config::MINT) != *mint.address() {
            return Err(EscrowError::WrongAccount.into());
        }
        (d[config::BUMP], read_addr(&d, config::OPERATOR), read_i64(&d, config::REFUND_GRACE))
    };
    let amount = {
        let mut d = ord.try_borrow_mut()?;
        if d[order::STATUS] != status::FUNDED {
            return Err(EscrowError::BadStatus.into());
        }
        if read_addr(&d, order::PAYER_TOKEN) != *payer_token.address() {
            return Err(EscrowError::WrongAccount.into());
        }
        if *caller.address() != operator {
            let open_until = read_i64(&d, order::DEADLINE).checked_add(grace).ok_or(ProgramError::ArithmeticOverflow)?;
            if now()? <= open_until {
                return Err(EscrowError::RefundNotAllowed.into());
            }
        }
        d[order::STATUS] = status::REFUNDED;
        read_u64(&d, order::AMOUNT)
    };
    let decimals = mint_decimals(mint)?;
    let b = [bump];
    transfer_checked(vault, mint, payer_token, cfg, amount, decimals, Some(&[Seed::from(config::SEED), Seed::from(&b)]))
}

fn close_order(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let [ord, rent_payer, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_state(ord, program_id, order::DISC, order::LEN)?;
    {
        let d = ord.try_borrow()?;
        if d[order::STATUS] == status::FUNDED {
            return Err(EscrowError::BadStatus.into());
        }
        if read_addr(&d, order::RENT_PAYER) != *rent_payer.address() {
            return Err(EscrowError::WrongAccount.into());
        }
    }
    close_account(ord, rent_payer)
}

fn set_config(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let new_admin = r.bytes32()?;
    let new_operator = r.bytes32()?;
    let burn_bps = r.u16()?;
    let treasury_bps = r.u16()?;
    let refund_grace = r.i64()?;
    r.finish()?;
    check_split(burn_bps, treasury_bps)?;
    if refund_grace < 0 {
        return Err(EscrowError::InvalidParams.into());
    }
    let [admin, cfg, payout, treasury, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_state(cfg, program_id, config::DISC, config::LEN)?;
    let mint = read_addr(&cfg.try_borrow()?, config::MINT);
    check_token_of(payout, &mint)?;
    check_token_of(treasury, &mint)?;
    let mut d = cfg.try_borrow_mut()?;
    if read_addr(&d, config::ADMIN) != *admin.address() {
        return Err(EscrowError::Unauthorized.into());
    }
    write_bytes(&mut d, config::ADMIN, &new_admin);
    write_bytes(&mut d, config::OPERATOR, &new_operator);
    write_bytes(&mut d, config::PAYOUT, payout.address().as_array());
    write_bytes(&mut d, config::TREASURY, treasury.address().as_array());
    write_u16(&mut d, config::BURN_BPS, burn_bps);
    write_u16(&mut d, config::TREASURY_BPS, treasury_bps);
    write_i64(&mut d, config::REFUND_GRACE, refund_grace);
    Ok(())
}

fn set_mint(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let [admin, cfg, old_vault, new_vault, new_mint, payout, treasury, system_program, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_system_program(system_program)?;
    require_token_program(token_program)?;
    require_state(cfg, program_id, config::DISC, config::LEN)?;
    {
        let d = cfg.try_borrow()?;
        if read_addr(&d, config::ADMIN) != *admin.address() {
            return Err(EscrowError::Unauthorized.into());
        }
        if read_addr(&d, config::VAULT) != *old_vault.address() {
            return Err(EscrowError::WrongAccount.into());
        }
        if read_addr(&d, config::MINT) == *new_mint.address() {
            return Err(EscrowError::InvalidParams.into());
        }
    }
    let (_, _, held) = token_account_info(old_vault)?;
    if held != 0 {
        return Err(EscrowError::VaultNotEmpty.into());
    }
    mint_decimals(new_mint)?;
    check_token_of(payout, new_mint.address())?;
    check_token_of(treasury, new_mint.address())?;
    let (vault_key, vault_bump) = find_pda(&[SEED_VAULT, new_mint.address().as_ref()], program_id);
    require_key(new_vault, &vault_key)?;
    if !new_vault.is_data_empty() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let vb = [vault_bump];
    create_vault(admin, new_vault, new_mint, cfg.address(), &[Seed::from(SEED_VAULT), Seed::from(new_mint.address().as_ref()), Seed::from(&vb)])?;

    let mut d = cfg.try_borrow_mut()?;
    d[config::VAULT_BUMP] = vault_bump;
    write_bytes(&mut d, config::MINT, new_mint.address().as_array());
    write_bytes(&mut d, config::VAULT, vault_key.as_array());
    write_bytes(&mut d, config::PAYOUT, payout.address().as_array());
    write_bytes(&mut d, config::TREASURY, treasury.address().as_array());
    Ok(())
}
