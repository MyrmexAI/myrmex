//! `treasury_split` — деление комиссий токена.
//!
//! В хранилище `Split` приходят комиссии пула. `distribute` может вызвать кто угодно: баланс
//! хранилища делится между получателями по `bps` (сумма ровно 10 000); получатель с нулевым адресом
//! означает сжигание. Остаток от округления остаётся до следующего раза.
//! Стартовые доли: байбек и сжигание 50%, места по работе 20%, места за присутствие 10%,
//! казна 10%, оркестратор 10%.

use pinocchio::{AccountView, Address, ProgramResult, cpi::Seed, error::ProgramError};
use shank::{ShankInstruction, ShankType};
use swarm_program_utils::*;

solana_address::declare_id!("AyUQQGc7m7PMZ4p9UP873YMLEz3Wrr21TxuBXC5F2yYH");

#[cfg(target_os = "solana")]
pinocchio::entrypoint!(process_instruction);

pub mod split {
    pub const DISC: u8 = 1;
    pub const SEED: &[u8] = b"split";
    pub const BUMP: usize = 2;
    pub const VAULT_BUMP: usize = 3;
    pub const COUNT: usize = 4;
    pub const ADMIN: usize = 8;
    pub const MINT: usize = 40;
    pub const VAULT: usize = 72;
    pub const TOTAL_DISTRIBUTED: usize = 104;
    pub const RECIPIENTS: usize = 112; // count × (dest 32 | bps u16 | pad 6)
    pub const RECIPIENT_LEN: usize = 40;
    pub const MAX_RECIPIENTS: usize = 8;
    pub const LEN: usize = RECIPIENTS + MAX_RECIPIENTS * RECIPIENT_LEN;
}

pub const SEED_VAULT: &[u8] = b"vault";
pub const BURN: [u8; 32] = [0u8; 32];

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitError {
    Unauthorized = 0,
    BadShares = 1,
    WrongAccount = 2,
    NothingToDistribute = 3,
    VaultNotEmpty = 4,
}

impl From<SplitError> for ProgramError {
    fn from(e: SplitError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

/// Получатель доли (Borsh: dest[32] | bps u16). dest = нули — сжигание.
#[derive(ShankType)]
pub struct Recipient {
    pub dest: [u8; 32],
    pub bps: u16,
}

#[derive(ShankInstruction)]
#[rustfmt::skip]
pub enum SplitInstruction {
    #[account(0, writable, signer, name = "admin")]
    #[account(1, writable, name = "split")]
    #[account(2, writable, name = "vault")]
    #[account(3, name = "mint")]
    #[account(4, name = "system_program")]
    #[account(5, name = "token_program")]
    Initialize { recipients: Vec<Recipient> },

    #[account(0, writable, name = "split")]
    #[account(1, writable, name = "vault")]
    #[account(2, writable, name = "mint")]
    #[account(3, name = "token_program")]
    Distribute,

    #[account(0, signer, name = "admin")]
    #[account(1, writable, name = "split")]
    SetRecipients { admin: [u8; 32], recipients: Vec<Recipient> },

    /// Смена токена после деплоя: только админ, только при пустом хранилище. Новое хранилище — PDA `["vault", new_mint]`;
    /// получатели сбрасываются (их токен-аккаунты были старого токена) — затем SetRecipients с аккаунтами нового.
    #[account(0, writable, signer, name = "admin")]
    #[account(1, writable, name = "split")]
    #[account(2, name = "old_vault")]
    #[account(3, writable, name = "new_vault")]
    #[account(4, name = "new_mint")]
    #[account(5, name = "system_program")]
    #[account(6, name = "token_program")]
    SetMint,
}

pub mod ix {
    pub const INITIALIZE: u8 = 0;
    pub const DISTRIBUTE: u8 = 1;
    pub const SET_RECIPIENTS: u8 = 2;
    pub const SET_MINT: u8 = 3;
}

pub fn process_instruction(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let (disc, rest) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    let mut r = Reader::new(rest);
    match *disc {
        ix::INITIALIZE => initialize(program_id, accounts, &mut r),
        ix::DISTRIBUTE => distribute(program_id, accounts, &r),
        ix::SET_RECIPIENTS => set_recipients(program_id, accounts, &mut r),
        ix::SET_MINT => set_mint(program_id, accounts, &r),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

/// Читает получателей, проверяет сумму долей и пишет их в аккаунт.
fn write_recipients(d: &mut [u8], r: &mut Reader) -> ProgramResult {
    let n = r.u32()? as usize;
    if n == 0 || n > split::MAX_RECIPIENTS {
        return Err(SplitError::BadShares.into());
    }
    let mut total = 0u32;
    d[split::RECIPIENTS..split::LEN].fill(0);
    for i in 0..n {
        let dest = r.bytes32()?;
        let bps = r.u16()?;
        if bps == 0 {
            return Err(SplitError::BadShares.into());
        }
        total += bps as u32;
        let o = split::RECIPIENTS + i * split::RECIPIENT_LEN;
        write_bytes(d, o, &dest);
        write_u16(d, o + 32, bps);
    }
    r.finish()?;
    if total != 10_000 {
        return Err(SplitError::BadShares.into());
    }
    d[split::COUNT] = n as u8;
    Ok(())
}

fn initialize(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let [admin, s_acc, vault, mint, system_program, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_system_program(system_program)?;
    require_token_program(token_program)?;
    mint_decimals(mint)?;
    let (key, bump) = find_pda(&[split::SEED], program_id);
    require_key(s_acc, &key)?;
    let (v_key, vbump) = find_pda(&[SEED_VAULT], program_id);
    require_key(vault, &v_key)?;
    if !s_acc.is_data_empty() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let b = [bump];
    create_pda_account(admin, s_acc, split::LEN, program_id, &[Seed::from(split::SEED), Seed::from(&b)])?;
    let vb = [vbump];
    create_vault(admin, vault, mint, &key, &[Seed::from(SEED_VAULT), Seed::from(&vb)])?;
    let mut d = s_acc.try_borrow_mut()?;
    d[0] = split::DISC;
    d[1] = 1;
    d[split::BUMP] = bump;
    d[split::VAULT_BUMP] = vbump;
    write_bytes(&mut d, split::ADMIN, admin.address().as_array());
    write_bytes(&mut d, split::MINT, mint.address().as_array());
    write_bytes(&mut d, split::VAULT, v_key.as_array());
    write_recipients(&mut d, r)
}

fn distribute(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let (head, dests) = accounts.split_at_mut(4);
    let [s_acc, vault, mint, token_program] = head else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_token_program(token_program)?;
    require_state(s_acc, program_id, split::DISC, split::LEN)?;
    let (bump, n) = {
        let d = s_acc.try_borrow()?;
        if read_addr(&d, split::VAULT) != *vault.address() || read_addr(&d, split::MINT) != *mint.address() {
            return Err(SplitError::WrongAccount.into());
        }
        (d[split::BUMP], d[split::COUNT] as usize)
    };
    let (_, _, balance) = token_account_info(vault)?;
    if balance == 0 {
        return Err(SplitError::NothingToDistribute.into());
    }
    let decimals = mint_decimals(mint)?;
    let b = [bump];
    let seeds = [Seed::from(split::SEED), Seed::from(&b)];
    let mut next_dest = 0usize;
    let mut sent = 0u64;
    for i in 0..n {
        let (dest, bps) = {
            let d = s_acc.try_borrow()?;
            let o = split::RECIPIENTS + i * split::RECIPIENT_LEN;
            (read_bytes32(&d, o), read_u16(&d, o + 32))
        };
        let amount = bps_of(balance, bps)?;
        if amount == 0 {
            continue;
        }
        if dest == BURN {
            burn_checked(vault, mint, s_acc, amount, decimals, &seeds)?;
        } else {
            let to = dests.get(next_dest).ok_or(ProgramError::NotEnoughAccountKeys)?;
            next_dest += 1;
            if to.address().as_array() != &dest {
                return Err(SplitError::WrongAccount.into());
            }
            transfer_checked(vault, mint, to, s_acc, amount, decimals, Some(&seeds))?;
        }
        sent = sent.checked_add(amount).ok_or(ProgramError::ArithmeticOverflow)?;
    }
    let mut d = s_acc.try_borrow_mut()?;
    let total = read_u64(&d, split::TOTAL_DISTRIBUTED).checked_add(sent).ok_or(ProgramError::ArithmeticOverflow)?;
    write_u64(&mut d, split::TOTAL_DISTRIBUTED, total);
    Ok(())
}

fn set_recipients(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let admin_new = r.bytes32()?;
    let [admin, s_acc, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_state(s_acc, program_id, split::DISC, split::LEN)?;
    let mut d = s_acc.try_borrow_mut()?;
    if read_addr(&d, split::ADMIN) != *admin.address() {
        return Err(SplitError::Unauthorized.into());
    }
    write_bytes(&mut d, split::ADMIN, &admin_new);
    write_recipients(&mut d, r)
}

fn set_mint(program_id: &Address, accounts: &mut [AccountView], r: &Reader) -> ProgramResult {
    r.finish()?;
    let [admin, s_acc, old_vault, new_vault, new_mint, system_program, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_system_program(system_program)?;
    require_token_program(token_program)?;
    require_state(s_acc, program_id, split::DISC, split::LEN)?;
    {
        let d = s_acc.try_borrow()?;
        if read_addr(&d, split::ADMIN) != *admin.address() {
            return Err(SplitError::Unauthorized.into());
        }
        if read_addr(&d, split::VAULT) != *old_vault.address() || read_addr(&d, split::MINT) == *new_mint.address() {
            return Err(SplitError::WrongAccount.into());
        }
    }
    let (_, _, held) = token_account_info(old_vault)?;
    if held != 0 {
        return Err(SplitError::VaultNotEmpty.into());
    }
    mint_decimals(new_mint)?;
    let (v_key, vbump) = find_pda(&[SEED_VAULT, new_mint.address().as_ref()], program_id);
    require_key(new_vault, &v_key)?;
    if !new_vault.is_data_empty() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let vb = [vbump];
    create_vault(admin, new_vault, new_mint, s_acc.address(), &[Seed::from(SEED_VAULT), Seed::from(new_mint.address().as_ref()), Seed::from(&vb)])?;
    let mut d = s_acc.try_borrow_mut()?;
    d[split::VAULT_BUMP] = vbump;
    d[split::COUNT] = 0;
    write_bytes(&mut d, split::MINT, new_mint.address().as_array());
    write_bytes(&mut d, split::VAULT, v_key.as_array());
    Ok(())
}
