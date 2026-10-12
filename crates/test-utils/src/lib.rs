//! Хелперы тестов программ роя на LiteSVM.

use litesvm::LiteSVM;
use solana_account::Account;
use solana_clock::Clock;
use solana_instruction::{Instruction, error::InstructionError};
use solana_keypair::Keypair;
use solana_program_pack::Pack;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
pub use solana_transaction::TransactionError;
use spl_token_interface::state::{Account as TokenAccount, AccountState, Mint};

pub use spl_token_interface::ID as TOKEN_PROGRAM_ID;
/// Token-2022.
pub const TOKEN_2022_PROGRAM_ID: Pubkey = Pubkey::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
pub const SYSTEM_PROGRAM_ID: Pubkey = Pubkey::new_from_array([0u8; 32]);

/// Каталог собранных программ (`cargo build-sbf` → target/deploy).
pub fn so_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::var("SBF_OUT_DIR").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/deploy").to_string()
    });
    std::path::Path::new(&dir).join(format!("{name}.so"))
}

pub struct Env {
    pub svm: LiteSVM,
    pub payer: Keypair,
}

impl Env {
    /// LiteSVM с программами `(id, имя .so)` и пополненным плательщиком.
    pub fn new(programs: &[(Pubkey, &str)]) -> Self {
        let mut svm = LiteSVM::new();
        for (id, name) in programs {
            svm.add_program_from_file(*id, so_path(name))
                .unwrap_or_else(|e| panic!("нет {name}.so — сначала `cargo build-sbf`: {e:?}"));
        }
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
        Self { svm, payer }
    }

    pub fn funded(&mut self) -> Keypair {
        let k = Keypair::new();
        self.svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        k
    }

    /// Отправить инструкции; плательщик — `self.payer`, плюс дополнительные подписанты.
    pub fn send(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<u64, TransactionError> {
        let mut all: Vec<&Keypair> = vec![&self.payer];
        all.extend_from_slice(signers);
        let tx = Transaction::new_signed_with_payer(ixs, Some(&self.payer.pubkey()), &all, self.svm.latest_blockhash());
        let res = self.svm.send_transaction(tx).map(|m| m.compute_units_consumed).map_err(|f| {
            eprintln!("{}", f.meta.logs.join("\n"));
            f.err
        });
        self.svm.expire_blockhash();
        res
    }

    pub fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    pub fn warp_seconds(&mut self, secs: i64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp += secs;
        clock.slot += (secs.max(0) as u64) * 5 / 2;
        self.svm.set_sysvar(&clock);
    }

    pub fn create_mint(&mut self, authority: &Pubkey, decimals: u8) -> Pubkey {
        self.create_mint_in(authority, decimals, TOKEN_PROGRAM_ID)
    }

    /// Mint Token-2022 (как у pump.fun `create_v2`; без расширений — та же раскладка 82 байта).
    pub fn create_mint_2022(&mut self, authority: &Pubkey, decimals: u8) -> Pubkey {
        self.create_mint_in(authority, decimals, TOKEN_2022_PROGRAM_ID)
    }

    /// Mint Token-2022 с одним расширением (TLV): `[база 82 | паддинг до 165 | тип аккаунта 1 | тип u16 | длина u16 | данные]`.
    pub fn create_mint_2022_with_ext(&mut self, authority: &Pubkey, decimals: u8, ext_type: u16, ext_data: &[u8]) -> Pubkey {
        let mint = self.create_mint_2022(authority, decimals);
        let mut acc = self.svm.get_account(&mint).unwrap();
        acc.data.resize(165, 0);
        acc.data.push(1);
        acc.data.extend_from_slice(&ext_type.to_le_bytes());
        acc.data.extend_from_slice(&(ext_data.len() as u16).to_le_bytes());
        acc.data.extend_from_slice(ext_data);
        acc.lamports = self.svm.minimum_balance_for_rent_exemption(acc.data.len());
        self.svm.set_account(mint, acc).unwrap();
        mint
    }

    fn create_mint_in(&mut self, authority: &Pubkey, decimals: u8, program: Pubkey) -> Pubkey {
        let mint = Pubkey::new_unique();
        let mut data = vec![0u8; Mint::LEN];
        Mint {
            mint_authority: Some(*authority).into(),
            supply: 0,
            decimals,
            is_initialized: true,
            freeze_authority: None.into(),
        }
        .pack_into_slice(&mut data);
        self.put(mint, data, program);
        mint
    }

    /// Токен-аккаунт с заданным балансом (состояние ставится напрямую).
    pub fn create_token_account(&mut self, mint: &Pubkey, owner: &Pubkey, amount: u64) -> Pubkey {
        let key = Pubkey::new_unique();
        self.set_token_account(key, mint, owner, amount);
        key
    }

    /// Токен-аккаунт Token-2022 (165 байт, без расширений).
    pub fn create_token_account_2022(&mut self, mint: &Pubkey, owner: &Pubkey, amount: u64) -> Pubkey {
        let key = Pubkey::new_unique();
        self.set_token_account(key, mint, owner, amount);
        let mut acc = self.svm.get_account(&key).unwrap();
        acc.owner = TOKEN_2022_PROGRAM_ID;
        self.svm.set_account(key, acc).unwrap();
        key
    }

    pub fn set_token_account(&mut self, key: Pubkey, mint: &Pubkey, owner: &Pubkey, amount: u64) {
        let mut data = vec![0u8; TokenAccount::LEN];
        TokenAccount {
            mint: *mint,
            owner: *owner,
            amount,
            delegate: None.into(),
            state: AccountState::Initialized,
            is_native: None.into(),
            delegated_amount: 0,
            close_authority: None.into(),
        }
        .pack_into_slice(&mut data);
        self.put(key, data, TOKEN_PROGRAM_ID);
    }

    pub fn token_balance(&self, key: &Pubkey) -> u64 {
        let acc = self.svm.get_account(key).expect("нет токен-аккаунта");
        TokenAccount::unpack(&acc.data).unwrap().amount
    }

    pub fn data(&self, key: &Pubkey) -> Vec<u8> {
        self.svm.get_account(key).map(|a| a.data).unwrap_or_default()
    }

    pub fn exists(&self, key: &Pubkey) -> bool {
        self.svm.get_account(key).is_some_and(|a| a.lamports > 0)
    }

    fn put(&mut self, key: Pubkey, data: Vec<u8>, owner: Pubkey) {
        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm
            .set_account(key, Account { lamports, data, owner, executable: false, rent_epoch: 0 })
            .unwrap();
    }
}

/// Ожидаемая ошибка `Custom(code)` в инструкции `index`.
pub fn custom_err(index: u8, code: u32) -> TransactionError {
    TransactionError::InstructionError(index, InstructionError::Custom(code))
}

pub fn ix_err(index: u8, e: InstructionError) -> TransactionError {
    TransactionError::InstructionError(index, e)
}

pub fn read_u64(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}
pub fn read_i64(d: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}
pub fn read_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

/// Деревья Меркла по тем же правилам, что packages/protocol/src/merkle.ts и программы.
pub mod merkle {
    use sha2::{Digest, Sha256};

    pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
        let mut h = Sha256::new();
        for p in parts {
            h.update(p);
        }
        h.finalize().into()
    }
    pub fn record_leaf(job_id: &[u8; 16], doc_hash: &[u8; 32]) -> [u8; 32] {
        sha256(&[&[0x00], job_id, doc_hash])
    }
    pub fn claim_leaf(wallet: &[u8; 32], cumulative: u64) -> [u8; 32] {
        sha256(&[&[0x02], wallet, &cumulative.to_le_bytes()])
    }
    pub fn hash_pair(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        sha256(&[&[0x01], lo, hi])
    }
    /// (корень, слои). Листья сортируются; нечётный узел поднимается без пары.
    pub fn build(leaves: &[[u8; 32]]) -> ([u8; 32], Vec<Vec<[u8; 32]>>) {
        let mut layer = leaves.to_vec();
        layer.sort();
        let mut layers = vec![layer];
        while layers.last().unwrap().len() > 1 {
            let prev = layers.last().unwrap();
            let next = prev.chunks(2).map(|c| if c.len() == 2 { hash_pair(&c[0], &c[1]) } else { c[0] }).collect();
            layers.push(next);
        }
        (layers.last().unwrap()[0], layers)
    }
    pub fn proof(layers: &[Vec<[u8; 32]>], leaf: &[u8; 32]) -> Vec<[u8; 32]> {
        let mut idx = layers[0].iter().position(|l| l == leaf).expect("лист не найден");
        let mut out = vec![];
        for layer in &layers[..layers.len() - 1] {
            let sib = idx ^ 1;
            if sib < layer.len() {
                out.push(layer[sib]);
            }
            idx >>= 1;
        }
        out
    }
}
