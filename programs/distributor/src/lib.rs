//! `distributor` — выплаты холдерам через Merkle-claim.
//!
//! Каждое распределение (`Distribution`, PDA `["dist", id]`) — свой mint и своё хранилище: id 0 —
//! заработок мест в нашем токене, остальные — доли роя с запусков токенов (раздел 11.3).
//! Казначей (`updater`) публикует корень дерева кумулятивных балансов
//! `leaf = sha256(0x02 || wallet || cumulative_amount u64 LE)`; холдер забирает разницу
//! `cumulative − already_claimed`. Автозабор: `updater` может забрать за холдера, но только на
//! токен-аккаунт, которым владеет сам холдер.

use pinocchio::{AccountView, Address, ProgramResult, cpi::Seed, error::ProgramError};
use shank::ShankInstruction;
use swarm_program_utils::*;

solana_address::declare_id!("9ukbQVEVkCvFV1rJyVnQbhM5WHqfcWSZRWQ9ZKcShFRZ");

#[cfg(target_os = "solana")]
pinocchio::entrypoint!(process_instruction);

pub mod dist {
    pub const DISC: u8 = 1;
    pub const SEED: &[u8] = b"dist";
    pub const BUMP: usize = 2;
    pub const VAULT_BUMP: usize = 3;
    pub const ID: usize = 8;
    pub const ADMIN: usize = 16;
    pub const UPDATER: usize = 48;
    pub const MINT: usize = 80;
    pub const VAULT: usize = 112;
    pub const ROOT: usize = 144;
    pub const EPOCH: usize = 176;
    pub const TOTAL_CLAIMED: usize = 184;
    pub const UPDATED_AT: usize = 192;
    pub const LEN: usize = 200;
}

pub mod claim {
    pub const DISC: u8 = 2;
    pub const SEED: &[u8] = b"claim";
    pub const BUMP: usize = 2;
    pub const CLAIMED: usize = 8;
    pub const WALLET: usize = 16;
    pub const LEN: usize = 48;
}

pub const SEED_VAULT: &[u8] = b"vault";

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistError {
    Unauthorized = 0,
    StaleEpoch = 1,
    InvalidProof = 2,
    NothingToClaim = 3,
    WrongAccount = 4,
    NoRoot = 5,
}

impl From<DistError> for ProgramError {
    fn from(e: DistError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

#[derive(ShankInstruction)]
#[rustfmt::skip]
pub enum DistributorInstruction {
    #[account(0, writable, signer, name = "admin")]
    #[account(1, writable, name = "distribution")]
    #[account(2, writable, name = "vault")]
    #[account(3, name = "mint")]
    #[account(4, name = "updater")]
    #[account(5, name = "system_program")]
    #[account(6, name = "token_program")]
    Create { id: u64 },

    #[account(0, signer, name = "updater")]
    #[account(1, writable, name = "distribution")]
    SetRoot { root: [u8; 32], epoch: u64 },

    #[account(0, writable, signer, name = "signer")]
    #[account(1, name = "wallet")]
    #[account(2, writable, name = "distribution")]
    #[account(3, writable, name = "claim_status")]
    #[account(4, writable, name = "vault")]
    #[account(5, writable, name = "wallet_token")]
    #[account(6, name = "mint")]
    #[account(7, name = "system_program")]
    #[account(8, name = "token_program")]
    Claim { cumulative_amount: u64, proof: Vec<[u8; 32]> },

    #[account(0, signer, name = "admin")]
    #[account(1, writable, name = "distribution")]
    SetAuthorities { admin: [u8; 32], updater: [u8; 32] },
}

pub mod ix {
    pub const CREATE: u8 = 0;
    pub const SET_ROOT: u8 = 1;
    pub const CLAIM: u8 = 2;
    pub const SET_AUTHORITIES: u8 = 3;
}

pub fn process_instruction(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let (disc, rest) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    let mut r = Reader::new(rest);
    match *disc {
        ix::CREATE => create(program_id, accounts, &mut r),
        ix::SET_ROOT => set_root(program_id, accounts, &mut r),
        ix::CLAIM => claim_ix(program_id, accounts, &mut r),
        ix::SET_AUTHORITIES => set_authorities(program_id, accounts, &mut r),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn create(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let id = r.u64()?;
    r.finish()?;
    let [admin, d_acc, vault, mint, updater, system_program, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_system_program(system_program)?;
    require_token_program(token_program)?;
    mint_decimals(mint)?;
    let id_b = id.to_le_bytes();
    let (d_key, bump) = find_pda(&[dist::SEED, &id_b], program_id);
    require_key(d_acc, &d_key)?;
    let (v_key, vbump) = find_pda(&[SEED_VAULT, d_key.as_array()], program_id);
    require_key(vault, &v_key)?;
    if !d_acc.is_data_empty() {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let b = [bump];
    create_pda_account(admin, d_acc, dist::LEN, program_id, &[Seed::from(dist::SEED), Seed::from(&id_b), Seed::from(&b)])?;
    let vb = [vbump];
    create_vault(admin, vault, mint, &d_key, &[Seed::from(SEED_VAULT), Seed::from(d_key.as_array()), Seed::from(&vb)])?;
    let mut d = d_acc.try_borrow_mut()?;
    d[0] = dist::DISC;
    d[1] = 1;
    d[dist::BUMP] = bump;
    d[dist::VAULT_BUMP] = vbump;
    write_u64(&mut d, dist::ID, id);
    write_bytes(&mut d, dist::ADMIN, admin.address().as_array());
    write_bytes(&mut d, dist::UPDATER, updater.address().as_array());
    write_bytes(&mut d, dist::MINT, mint.address().as_array());
    write_bytes(&mut d, dist::VAULT, v_key.as_array());
    Ok(())
}

fn set_root(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let root = r.bytes32()?;
    let epoch = r.u64()?;
    r.finish()?;
    let [updater, d_acc, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(updater)?;
    require_state(d_acc, program_id, dist::DISC, dist::LEN)?;
    let now = now()?;
    let mut d = d_acc.try_borrow_mut()?;
    if read_addr(&d, dist::UPDATER) != *updater.address() {
        return Err(DistError::Unauthorized.into());
    }
    // эпохи только растут: старое дерево нельзя вернуть
    if epoch <= read_u64(&d, dist::EPOCH) {
        return Err(DistError::StaleEpoch.into());
    }
    write_bytes(&mut d, dist::ROOT, &root);
    write_u64(&mut d, dist::EPOCH, epoch);
    write_i64(&mut d, dist::UPDATED_AT, now);
    Ok(())
}

fn claim_ix(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let cumulative = r.u64()?;
    let n = r.u32()? as usize;
    let proof = r.take(n.checked_mul(32).ok_or(ProgramError::InvalidInstructionData)?)?;
    r.finish()?;
    let [signer, wallet, d_acc, status, vault, wallet_token, mint, system_program, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(signer)?;
    require_system_program(system_program)?;
    require_token_program(token_program)?;
    require_state(d_acc, program_id, dist::DISC, dist::LEN)?;

    let (root, d_bump, id) = {
        let d = d_acc.try_borrow()?;
        if *signer.address() != *wallet.address() && *signer.address() != read_addr(&d, dist::UPDATER) {
            return Err(DistError::Unauthorized.into());
        }
        if read_addr(&d, dist::VAULT) != *vault.address() || read_addr(&d, dist::MINT) != *mint.address() {
            return Err(DistError::WrongAccount.into());
        }
        if read_u64(&d, dist::EPOCH) == 0 {
            return Err(DistError::NoRoot.into());
        }
        (read_bytes32(&d, dist::ROOT), d[dist::BUMP], read_u64(&d, dist::ID))
    };
    let (t_mint, t_owner, _) = token_account_info(wallet_token)?;
    if t_mint != *mint.address() || t_owner != *wallet.address() {
        return Err(DistError::WrongAccount.into());
    }

    let leaf = sha256(&[&[LEAF_CLAIM], wallet.address().as_array(), &cumulative.to_le_bytes()]);
    if !verify_proof(leaf, proof, &root)? {
        return Err(DistError::InvalidProof.into());
    }

    let (s_key, s_bump) = find_pda(&[claim::SEED, d_acc.address().as_array(), wallet.address().as_array()], program_id);
    require_key(status, &s_key)?;
    if status.is_data_empty() {
        let sb = [s_bump];
        create_pda_account(
            signer,
            status,
            claim::LEN,
            program_id,
            &[Seed::from(claim::SEED), Seed::from(d_acc.address().as_array()), Seed::from(wallet.address().as_array()), Seed::from(&sb)],
        )?;
        let mut s = status.try_borrow_mut()?;
        s[0] = claim::DISC;
        s[1] = 1;
        s[claim::BUMP] = s_bump;
        write_bytes(&mut s, claim::WALLET, wallet.address().as_array());
    } else {
        require_state(status, program_id, claim::DISC, claim::LEN)?;
    }

    let amount = {
        let mut s = status.try_borrow_mut()?;
        let claimed = read_u64(&s, claim::CLAIMED);
        let amount = cumulative.checked_sub(claimed).ok_or(DistError::NothingToClaim)?;
        if amount == 0 {
            return Err(DistError::NothingToClaim.into());
        }
        write_u64(&mut s, claim::CLAIMED, cumulative);
        amount
    };
    {
        let mut d = d_acc.try_borrow_mut()?;
        let total = read_u64(&d, dist::TOTAL_CLAIMED).checked_add(amount).ok_or(ProgramError::ArithmeticOverflow)?;
        write_u64(&mut d, dist::TOTAL_CLAIMED, total);
    }
    let decimals = mint_decimals(mint)?;
    let id_b = id.to_le_bytes();
    let b = [d_bump];
    transfer_checked(vault, mint, wallet_token, d_acc, amount, decimals, Some(&[Seed::from(dist::SEED), Seed::from(&id_b), Seed::from(&b)]))
}

fn set_authorities(program_id: &Address, accounts: &mut [AccountView], r: &mut Reader) -> ProgramResult {
    let admin_new = r.bytes32()?;
    let updater_new = r.bytes32()?;
    r.finish()?;
    let [admin, d_acc, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    require_signer(admin)?;
    require_state(d_acc, program_id, dist::DISC, dist::LEN)?;
    let mut d = d_acc.try_borrow_mut()?;
    if read_addr(&d, dist::ADMIN) != *admin.address() {
        return Err(DistError::Unauthorized.into());
    }
    write_bytes(&mut d, dist::ADMIN, &admin_new);
    write_bytes(&mut d, dist::UPDATER, &updater_new);
    Ok(())
}
