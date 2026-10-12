//! Раскладка аккаунтов `seat_registry`. Смещения — единственный источник правды;
//! TS-клиент и сервер читают те же поля (см. IDL).

/// `Config` — PDA `["config"]`.
pub mod config {
    pub const DISC: u8 = 1;
    pub const VERSION: u8 = 1;
    pub const SEED: &[u8] = b"config";

    pub const BUMP: usize = 2;
    pub const VAULT_BUMP: usize = 3;
    pub const MAX_SEATS: usize = 4; // мест на кошелёк
    pub const HAS_PENDING: usize = 5;
    pub const ADMIN: usize = 8;
    pub const MINT: usize = 40;
    pub const VAULT: usize = 72;
    pub const TREASURY: usize = 104; // токен-аккаунт, куда идут штрафы
    pub const SLASH_AUTHORITY: usize = 136;
    pub const STAKE_AMOUNT: usize = 168;
    pub const UNLOCK_DELAY: usize = 176; // секунды
    pub const TIMELOCK: usize = 184; // секунды, для штрафов и смены конфига
    pub const MAX_SLASH_BPS: usize = 192; // u16
    pub const SEATS_OPEN: usize = 200; // u64: открытых мест сейчас (open +1, withdraw −1)
    /// Всего мест в сети (решение владельца 2026-10-10): open_seat сверх лимита — SeatsFull.
    pub const MAX_TOTAL_SEATS: u64 = 500;
    // ожидающая смена конфига
    pub const P_ADMIN: usize = 208;
    pub const P_SLASH_AUTHORITY: usize = 240;
    pub const P_TREASURY: usize = 272;
    pub const P_STAKE_AMOUNT: usize = 304;
    pub const P_UNLOCK_DELAY: usize = 312;
    pub const P_MAX_SEATS: usize = 320; // u8
    pub const P_EFFECTIVE_AT: usize = 328;
    pub const LEN: usize = 336;
}

/// `Seat` — PDA `["seat", owner, index]`.
pub mod seat {
    pub const DISC: u8 = 2;
    pub const VERSION: u8 = 1;
    pub const SEED: &[u8] = b"seat";

    pub const BUMP: usize = 2;
    pub const INDEX: usize = 3;
    pub const STATUS: usize = 4;
    pub const OWNER: usize = 8;
    pub const DEVICE_KEY: usize = 40; // Ed25519 pubkey устройства; нули — не привязано
    pub const STAKED: usize = 72;
    pub const CREATED_AT: usize = 80;
    pub const UNLOCK_REQUESTED_AT: usize = 88;
    pub const PENDING_SLASH_BPS: usize = 96; // u16; 0 — штрафа нет
    pub const SLASH_EXECUTABLE_AT: usize = 104;
    pub const LEN: usize = 112;
}

pub const SEED_VAULT: &[u8] = b"vault";

/// Статус места.
pub mod status {
    pub const ACTIVE: u8 = 0;
    pub const UNLOCKING: u8 = 1;
    pub const SLASHED: u8 = 2;
}
