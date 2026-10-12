//! Общие хелперы программ роя на Pinocchio.
//!
//! Данные аккаунтов читаются и пишутся по явным смещениям (little-endian), без приведения
//! указателей к структурам: так нет проблем с выравниванием и раскладка видна в одном месте.
//! Общая раскладка любого аккаунта программ: `[disc: u8 | version: u8 | bump: u8 | ...]`.

#![no_std]

use pinocchio::{
    AccountView, Address, ProgramResult,
    cpi::{Seed, Signer, invoke_signed},
    instruction::{InstructionAccount, InstructionView},
    error::ProgramError,
    sysvars::{Sysvar, clock::Clock, rent::Rent},
};
use pinocchio_system::instructions::{Allocate, Assign, Transfer};

pub use pinocchio_token::ID as TOKEN_PROGRAM_ID;
/// Token-2022 (`TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb`): pump.fun `create_v2` выпускает токены на нём.
pub const TOKEN_2022_PROGRAM_ID: Address = Address::new_from_array([
    6, 221, 246, 225, 238, 117, 143, 222, 24, 66, 93, 188, 228, 108, 205, 218, 182, 26, 252, 77, 131, 185, 13, 39, 254, 189, 249, 40, 216, 161, 139, 252,
]);
pub use pinocchio_system::ID as SYSTEM_PROGRAM_ID;

// ---------- проверки ----------

#[inline(always)]
pub fn require_signer(a: &AccountView) -> ProgramResult {
    if a.is_signer() { Ok(()) } else { Err(ProgramError::MissingRequiredSignature) }
}

#[inline(always)]
pub fn require_writable(a: &AccountView) -> ProgramResult {
    if a.is_writable() { Ok(()) } else { Err(ProgramError::InvalidAccountData) }
}

#[inline(always)]
pub fn require_owned_by(a: &AccountView, owner: &Address) -> ProgramResult {
    if a.owned_by(owner) { Ok(()) } else { Err(ProgramError::IncorrectProgramId) }
}

#[inline(always)]
pub fn require_key(a: &AccountView, key: &Address) -> ProgramResult {
    if a.address() == key { Ok(()) } else { Err(ProgramError::InvalidArgument) }
}

#[inline(always)]
pub fn require_token_program(a: &AccountView) -> ProgramResult {
    if a.address() == &TOKEN_PROGRAM_ID || a.address() == &TOKEN_2022_PROGRAM_ID { Ok(()) } else { Err(ProgramError::IncorrectProgramId) }
}

#[inline(always)]
pub fn require_system_program(a: &AccountView) -> ProgramResult {
    if a.address() == &SYSTEM_PROGRAM_ID { Ok(()) } else { Err(ProgramError::IncorrectProgramId) }
}

/// Аккаунт программы с ожидаемым дискриминатором и длиной.
pub fn require_state(a: &AccountView, program_id: &Address, disc: u8, len: usize) -> ProgramResult {
    require_owned_by(a, program_id)?;
    let data = a.try_borrow()?;
    if data.len() != len || data[0] != disc {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(())
}

// ---------- поля ----------

#[inline(always)]
pub fn read_u8(d: &[u8], o: usize) -> u8 {
    d[o]
}
#[inline(always)]
pub fn read_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
#[inline(always)]
pub fn read_u64(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}
#[inline(always)]
pub fn read_i64(d: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}
#[inline(always)]
pub fn read_addr(d: &[u8], o: usize) -> Address {
    Address::new_from_array(d[o..o + 32].try_into().unwrap())
}
#[inline(always)]
pub fn read_bytes32(d: &[u8], o: usize) -> [u8; 32] {
    d[o..o + 32].try_into().unwrap()
}
#[inline(always)]
pub fn write_u16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
#[inline(always)]
pub fn write_u64(d: &mut [u8], o: usize, v: u64) {
    d[o..o + 8].copy_from_slice(&v.to_le_bytes());
}
#[inline(always)]
pub fn write_i64(d: &mut [u8], o: usize, v: i64) {
    d[o..o + 8].copy_from_slice(&v.to_le_bytes());
}
#[inline(always)]
pub fn write_bytes(d: &mut [u8], o: usize, v: &[u8]) {
    d[o..o + v.len()].copy_from_slice(v);
}

/// Курсор по данным инструкции; любая нехватка байт — `InvalidInstructionData`.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], ProgramError> {
        let end = self.pos.checked_add(n).ok_or(ProgramError::InvalidInstructionData)?;
        let s = self.data.get(self.pos..end).ok_or(ProgramError::InvalidInstructionData)?;
        self.pos = end;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8, ProgramError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, ProgramError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, ProgramError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, ProgramError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn i64(&mut self) -> Result<i64, ProgramError> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn bytes32(&mut self) -> Result<[u8; 32], ProgramError> {
        Ok(self.take(32)?.try_into().unwrap())
    }
    pub fn addr(&mut self) -> Result<Address, ProgramError> {
        Ok(Address::new_from_array(self.bytes32()?))
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.data[self.pos..];
        self.pos = self.data.len();
        s
    }
    /// Данные должны быть прочитаны целиком.
    pub fn finish(&self) -> ProgramResult {
        if self.pos == self.data.len() { Ok(()) } else { Err(ProgramError::InvalidInstructionData) }
    }
}

// ---------- время ----------

pub fn now() -> Result<i64, ProgramError> {
    Ok(Clock::get()?.unix_timestamp)
}

// ---------- PDA ----------

/// Канонический PDA и bump. Только при создании аккаунта; дальше bump хранится в данных.
pub fn find_pda(seeds: &[&[u8]], program_id: &Address) -> (Address, u8) {
    Address::find_program_address(seeds, program_id)
}

/// Создаёт PDA-аккаунт с защитой от предварительного пополнения (lamport griefing):
/// переводится только недостающая до rent-exempt сумма, затем Allocate + Assign с подписью PDA.
pub fn create_pda_account(
    payer: &AccountView,
    account: &AccountView,
    space: usize,
    owner: &Address,
    signer_seeds: &[Seed],
) -> ProgramResult {
    let required = Rent::get()?.try_minimum_balance(space)?;
    let current = account.lamports();
    if current < required {
        Transfer { from: payer, to: account, lamports: required - current }.invoke()?;
    }
    let signer = [Signer::from(signer_seeds)];
    Allocate { account, space: space as u64 }.invoke_signed(&signer)?;
    Assign { account, owner }.invoke_signed(&signer)
}

/// Закрывает аккаунт программы: lamports получателю, данные обнуляются, владелец — System Program.
pub fn close_account(account: &mut AccountView, recipient: &mut AccountView) -> ProgramResult {
    let lamports = account.lamports();
    let total = recipient.lamports().checked_add(lamports).ok_or(ProgramError::ArithmeticOverflow)?;
    recipient.set_lamports(total);
    account.set_lamports(0);
    {
        let mut d = account.try_borrow_mut()?;
        d.fill(0);
    }
    account.close()
}

// ---------- токены (SPL Token и Token-2022) ----------
//
// Программа токенов берётся из владельца mint (его задаёт рантайм): классический SPL Token или Token-2022.
// Раскладка базовых полей у обеих одинаковая; у Token-2022 после 165 байт — тип аккаунта и расширения.
// Вызовы собираются вручную: в pinocchio-token адрес программы зашит (`crate::ID`).

/// Программа токенов, которой принадлежит аккаунт (mint или токен-аккаунт).
pub fn token_program_of(a: &AccountView) -> Result<&'static Address, ProgramError> {
    if a.owned_by(&TOKEN_PROGRAM_ID) {
        Ok(&TOKEN_PROGRAM_ID)
    } else if a.owned_by(&TOKEN_2022_PROGRAM_ID) {
        Ok(&TOKEN_2022_PROGRAM_ID)
    } else {
        Err(ProgramError::IncorrectProgramId)
    }
}

const ACCOUNT_TYPE_OFFSET: usize = 165; // Token-2022: 1 — mint, 2 — токен-аккаунт

/// Данные токен-аккаунта: (mint, owner, amount). Проверяет программу-владельца, размер и инициализацию.
pub fn token_account_info(a: &AccountView) -> Result<(Address, Address, u64), ProgramError> {
    token_program_of(a)?;
    let d = a.try_borrow()?;
    if d.len() < TOKEN_ACCOUNT_LEN || d[108] == 0 || (d.len() > TOKEN_ACCOUNT_LEN && d[ACCOUNT_TYPE_OFFSET] != 2) {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok((read_addr(&d, 0), read_addr(&d, 32), read_u64(&d, 64)))
}

/// Расширения Token-2022, ломающие учёт хранилищ: комиссия перевода (1), конфиденциальные переводы (4),
/// заморозка по умолчанию (6), непередаваемый (9), вечный делегат (12 — может вывести хранилище), хук перевода (14), пауза (26).
const UNSAFE_MINT_EXTENSIONS: [u16; 7] = [1, 4, 6, 9, 12, 14, 26];

/// Знаки mint: программа-владелец, инициализация; для Token-2022 — тип аккаунта и отказ от опасных расширений
/// (обычный токен pump.fun `create_v2` — только MetadataPointer + TokenMetadata — проходит).
pub fn mint_decimals(mint: &AccountView) -> Result<u8, ProgramError> {
    token_program_of(mint)?;
    let d = mint.try_borrow()?;
    if d.len() < 82 || d[45] != 1 {
        return Err(ProgramError::InvalidAccountData);
    }
    if d.len() > 82 {
        if d.len() <= ACCOUNT_TYPE_OFFSET || d[ACCOUNT_TYPE_OFFSET] != 1 {
            return Err(ProgramError::InvalidAccountData);
        }
        // TLV после типа аккаунта: [тип u16 | длина u16 | данные]
        let mut o = ACCOUNT_TYPE_OFFSET + 1;
        while o + 4 <= d.len() {
            let ty = read_u16(&d, o);
            let len = read_u16(&d, o + 2) as usize;
            if ty == 0 {
                break;
            }
            if UNSAFE_MINT_EXTENSIONS.contains(&ty) {
                return Err(ProgramError::InvalidAccountData);
            }
            o = o.checked_add(4 + len).ok_or(ProgramError::InvalidAccountData)?;
        }
    }
    Ok(d[44])
}

fn token_cpi<const N: usize>(program: &Address, metas: &[InstructionAccount; N], views: &[&AccountView; N], data: &[u8], signer_seeds: Option<&[Seed]>) -> ProgramResult {
    let ix = InstructionView { program_id: program, accounts: metas, data };
    match signer_seeds {
        Some(seeds) => invoke_signed(&ix, views, &[Signer::from(seeds)]),
        None => invoke_signed(&ix, views, &[]),
    }
}

pub fn transfer_checked(
    from: &AccountView,
    mint: &AccountView,
    to: &AccountView,
    authority: &AccountView,
    amount: u64,
    decimals: u8,
    signer_seeds: Option<&[Seed]>,
) -> ProgramResult {
    let mut data = [0u8; 10];
    data[0] = 12;
    data[1..9].copy_from_slice(&amount.to_le_bytes());
    data[9] = decimals;
    let metas = [
        InstructionAccount::writable(from.address()),
        InstructionAccount::readonly(mint.address()),
        InstructionAccount::writable(to.address()),
        InstructionAccount::readonly_signer(authority.address()),
    ];
    token_cpi(token_program_of(mint)?, &metas, &[from, mint, to, authority], &data, signer_seeds)
}

/// `amount * bps / 10_000` без переполнения.
pub fn bps_of(amount: u64, bps: u16) -> Result<u64, ProgramError> {
    let v = (amount as u128) * (bps as u128) / 10_000u128;
    u64::try_from(v).map_err(|_| ProgramError::ArithmeticOverflow)
}

/// Размер токен-аккаунта (SPL Token; Token-2022 без расширений аккаунта — тот же).
pub const TOKEN_ACCOUNT_LEN: usize = 165;

/// Инициализирует выделенный (165 байт, владелец — программа токенов mint) аккаунт как токен-аккаунт (InitializeAccount3).
/// Mint Token-2022 с расширениями, которые требуют расширений аккаунта (комиссия перевода, хук), — отклоняется программой токенов.
pub fn init_token_account(account: &AccountView, mint: &AccountView, owner: &Address) -> ProgramResult {
    let mut data = [0u8; 33];
    data[0] = 18;
    data[1..33].copy_from_slice(owner.as_ref());
    let metas = [InstructionAccount::writable(account.address()), InstructionAccount::readonly(mint.address())];
    token_cpi(token_program_of(mint)?, &metas, &[account, mint], &data, None)
}

/// Сжечь токены с аккаунта (authority — PDA программы).
pub fn burn_checked(
    account: &AccountView,
    mint: &AccountView,
    authority: &AccountView,
    amount: u64,
    decimals: u8,
    signer_seeds: &[Seed],
) -> ProgramResult {
    let mut data = [0u8; 10];
    data[0] = 15;
    data[1..9].copy_from_slice(&amount.to_le_bytes());
    data[9] = decimals;
    let metas = [
        InstructionAccount::writable(account.address()),
        InstructionAccount::writable(mint.address()),
        InstructionAccount::readonly_signer(authority.address()),
    ];
    token_cpi(token_program_of(mint)?, &metas, &[account, mint, authority], &data, Some(signer_seeds))
}

/// Создаёт PDA токен-аккаунт (хранилище) с владельцем `authority` в программе токенов этого mint.
pub fn create_vault(
    payer: &AccountView,
    vault: &AccountView,
    mint: &AccountView,
    authority: &Address,
    vault_seeds: &[Seed],
) -> ProgramResult {
    create_pda_account(payer, vault, TOKEN_ACCOUNT_LEN, token_program_of(mint)?, vault_seeds)?;
    init_token_account(vault, mint, authority)
}

// ---------- sha256 и деревья Меркла (правила как в packages/protocol/src/merkle.ts) ----------

/// sha256 над конкатенацией срезов (syscall `sol_sha256`). На хосте не используется.
pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut out = [0u8; 32];
    #[cfg(target_os = "solana")]
    unsafe {
        pinocchio::syscalls::sol_sha256(parts.as_ptr() as *const u8, parts.len() as u64, out.as_mut_ptr());
    }
    #[cfg(not(target_os = "solana"))]
    {
        let _ = parts;
        unimplemented!("sha256 доступен только в программе на Solana");
    }
    out
}

pub const LEAF_RECORD: u8 = 0x00;
pub const MERKLE_NODE: u8 = 0x01;
pub const LEAF_CLAIM: u8 = 0x02;

/// Внутренний узел: sha256(0x01 || min(a,b) || max(a,b)).
pub fn hash_pair(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    sha256(&[&[MERKLE_NODE], lo, hi])
}

/// Проверка доказательства; `proof` — подряд идущие 32-байтные узлы.
pub fn verify_proof(leaf: [u8; 32], proof: &[u8], root: &[u8; 32]) -> Result<bool, ProgramError> {
    if proof.len() % 32 != 0 || proof.len() > 32 * 40 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let mut h = leaf;
    for node in proof.chunks_exact(32) {
        h = hash_pair(&h, node.try_into().unwrap());
    }
    Ok(&h == root)
}
