//! Сценарии `seat_registry` на LiteSVM: открыть место → привязать устройство → разблокировка → вывод,
//! штраф с таймлоком и апелляцией, смена конфига через таймлок.

use seat_registry::{SeatError, ix, state::{config as c, seat as s, status, SEED_VAULT}};
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use swarm_test_utils::*;

const STAKE: u64 = 1_000_000_000; // 1000 токенов при 6 знаках
const DAY: i64 = 86_400;
const UNLOCK_DELAY: i64 = 7 * DAY;
const TIMELOCK: i64 = 2 * DAY;

fn pid() -> Pubkey {
    Pubkey::new_from_array(seat_registry::ID.to_bytes())
}
fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[c::SEED], &pid()).0
}
fn vault_pda() -> Pubkey {
    Pubkey::find_program_address(&[SEED_VAULT], &pid()).0
}
fn seat_pda(owner: &Pubkey, index: u8) -> Pubkey {
    Pubkey::find_program_address(&[s::SEED, owner.as_ref(), &[index]], &pid()).0
}
fn ixn(data: Vec<u8>, accounts: Vec<AccountMeta>) -> Instruction {
    Instruction { program_id: pid(), accounts, data }
}

struct World {
    env: Env,
    admin: Keypair,
    slasher: Keypair,
    mint: Pubkey,
    treasury: Pubkey,
}

impl World {
    fn new() -> Self {
        let mut env = Env::new(&[(pid(), "seat_registry")]);
        let admin = env.funded();
        let slasher = env.funded();
        let mint = env.create_mint(&admin.pubkey(), 6);
        let treasury = env.create_token_account(&mint, &admin.pubkey(), 0);
        let mut w = Self { env, admin, slasher, mint, treasury };
        let mut data = vec![ix::INITIALIZE];
        data.extend_from_slice(&STAKE.to_le_bytes());
        data.extend_from_slice(&UNLOCK_DELAY.to_le_bytes());
        data.extend_from_slice(&TIMELOCK.to_le_bytes());
        data.push(3);
        data.extend_from_slice(&1_000u16.to_le_bytes());
        let i = ixn(data, vec![
            AccountMeta::new(w.admin.pubkey(), true),
            AccountMeta::new(config_pda(), false),
            AccountMeta::new(vault_pda(), false),
            AccountMeta::new_readonly(w.mint, false),
            AccountMeta::new_readonly(w.treasury, false),
            AccountMeta::new_readonly(w.slasher.pubkey(), false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ]);
        let admin = w.admin.insecure_clone();
        w.env.send(&[i], &[&admin]).expect("initialize");
        w
    }

    /// Холдер с токенами на `n` залогов.
    fn holder(&mut self, n: u64) -> (Keypair, Pubkey) {
        let k = self.env.funded();
        let ata = self.env.create_token_account(&self.mint, &k.pubkey(), STAKE * n);
        (k, ata)
    }

    fn open_ix(&self, owner: &Pubkey, owner_token: &Pubkey, index: u8) -> Instruction {
        ixn(vec![ix::OPEN_SEAT, index], vec![
            AccountMeta::new(*owner, true),
            AccountMeta::new(config_pda(), false),
            AccountMeta::new(seat_pda(owner, index), false),
            AccountMeta::new(*owner_token, false),
            AccountMeta::new(vault_pda(), false),
            AccountMeta::new_readonly(self.mint, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ])
    }

    fn device_ix(disc: u8, owner: &Pubkey, seat: &Pubkey, key: [u8; 32]) -> Instruction {
        let mut data = vec![disc];
        data.extend_from_slice(&key);
        ixn(data, vec![AccountMeta::new_readonly(*owner, true), AccountMeta::new(*seat, false)])
    }

    fn unlock_ix(owner: &Pubkey, seat: &Pubkey) -> Instruction {
        ixn(vec![ix::REQUEST_UNLOCK], vec![AccountMeta::new_readonly(*owner, true), AccountMeta::new(*seat, false)])
    }

    fn withdraw_ix(&self, owner: &Pubkey, seat: &Pubkey, owner_token: &Pubkey) -> Instruction {
        ixn(vec![ix::WITHDRAW], vec![
            AccountMeta::new(*owner, true),
            AccountMeta::new(config_pda(), false),
            AccountMeta::new(*seat, false),
            AccountMeta::new(*owner_token, false),
            AccountMeta::new(vault_pda(), false),
            AccountMeta::new_readonly(self.mint, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ])
    }

    fn propose_slash_ix(authority: &Pubkey, seat: &Pubkey, bps: u16) -> Instruction {
        let mut data = vec![ix::PROPOSE_SLASH];
        data.extend_from_slice(&bps.to_le_bytes());
        ixn(data, vec![
            AccountMeta::new_readonly(*authority, true),
            AccountMeta::new_readonly(config_pda(), false),
            AccountMeta::new(*seat, false),
        ])
    }

    fn execute_slash_ix(&self, seat: &Pubkey) -> Instruction {
        ixn(vec![ix::EXECUTE_SLASH], vec![
            AccountMeta::new_readonly(config_pda(), false),
            AccountMeta::new(*seat, false),
            AccountMeta::new(vault_pda(), false),
            AccountMeta::new(self.treasury, false),
            AccountMeta::new_readonly(self.mint, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ])
    }
}

#[test]
fn open_bind_unlock_withdraw() {
    let mut w = World::new();
    let (holder, ata) = w.holder(2);
    let h = holder.pubkey();
    let seat = seat_pda(&h, 0);

    let cu = w.env.send(&[w.open_ix(&h, &ata, 0)], &[&holder]).unwrap();
    println!("open_seat: {cu} CU");
    assert_eq!(w.env.token_balance(&ata), STAKE);
    assert_eq!(w.env.token_balance(&vault_pda()), STAKE);
    let d = w.env.data(&seat);
    assert_eq!(d[s::STATUS], status::ACTIVE);
    assert_eq!(read_u64(&d, s::STAKED), STAKE);
    assert_eq!(read_u64(&w.env.data(&config_pda()), c::SEATS_OPEN), 1);

    // место с тем же индексом второй раз не открыть
    assert!(w.env.send(&[w.open_ix(&h, &ata, 0)], &[&holder]).is_err());

    let device = [7u8; 32];
    w.env.send(&[World::device_ix(ix::BIND_DEVICE, &h, &seat, device)], &[&holder]).unwrap();
    assert_eq!(&w.env.data(&seat)[s::DEVICE_KEY..s::DEVICE_KEY + 32], &device);
    assert_eq!(
        w.env.send(&[World::device_ix(ix::BIND_DEVICE, &h, &seat, [8u8; 32])], &[&holder]).unwrap_err(),
        custom_err(0, SeatError::DeviceAlreadyBound as u32)
    );
    w.env.send(&[World::device_ix(ix::ROTATE_DEVICE, &h, &seat, [8u8; 32])], &[&holder]).unwrap();

    // чужой не может менять устройство
    let stranger = w.env.funded();
    assert_eq!(
        w.env.send(&[World::device_ix(ix::ROTATE_DEVICE, &stranger.pubkey(), &seat, [9u8; 32])], &[&stranger]).unwrap_err(),
        custom_err(0, SeatError::WrongOwner as u32)
    );

    w.env.send(&[World::unlock_ix(&h, &seat)], &[&holder]).unwrap();
    let d = w.env.data(&seat);
    assert_eq!(d[s::STATUS], status::UNLOCKING);
    assert_eq!(&d[s::DEVICE_KEY..s::DEVICE_KEY + 32], &[0u8; 32], "устройство отвязано на время разблокировки");

    assert_eq!(
        w.env.send(&[w.withdraw_ix(&h, &seat, &ata)], &[&holder]).unwrap_err(),
        custom_err(0, SeatError::UnlockNotReady as u32)
    );
    w.env.warp_seconds(UNLOCK_DELAY);
    w.env.send(&[w.withdraw_ix(&h, &seat, &ata)], &[&holder]).unwrap();
    assert_eq!(w.env.token_balance(&ata), 2 * STAKE);
    assert!(!w.env.exists(&seat), "аккаунт места закрыт");
    assert_eq!(read_u64(&w.env.data(&config_pda()), c::SEATS_OPEN), 0);

    // место можно открыть заново
    w.env.send(&[w.open_ix(&h, &ata, 0)], &[&holder]).unwrap();
}

#[test]
fn open_with_amount_above_minimum_and_withdraw_it_back() {
    let mut w = World::new();
    let (holder, ata) = w.holder(3);
    let h = holder.pubkey();
    let seat = seat_pda(&h, 0);
    // сумма на $100 по рыночной цене больше минимума: блокируется ровно она и ровно она возвращается
    let mut open = w.open_ix(&h, &ata, 0);
    open.data.extend_from_slice(&(STAKE * 2 + 7).to_le_bytes());
    w.env.send(&[open], &[&holder]).unwrap();
    assert_eq!(w.env.token_balance(&vault_pda()), STAKE * 2 + 7);
    assert_eq!(read_u64(&w.env.data(&seat), s::STAKED), STAKE * 2 + 7);
    w.env.send(&[World::unlock_ix(&h, &seat)], &[&holder]).unwrap();
    w.env.warp_seconds(UNLOCK_DELAY);
    w.env.send(&[w.withdraw_ix(&h, &seat, &ata)], &[&holder]).unwrap();
    assert_eq!(w.env.token_balance(&ata), STAKE * 3);
}

#[test]
fn open_below_minimum_or_with_bad_data_is_rejected() {
    let mut w = World::new();
    let (holder, ata) = w.holder(2);
    let h = holder.pubkey();
    let mut low = w.open_ix(&h, &ata, 0);
    low.data.extend_from_slice(&(STAKE - 1).to_le_bytes());
    assert_eq!(w.env.send(&[low], &[&holder]).unwrap_err(), custom_err(0, SeatError::StakeTooLow as u32));
    let mut bad = w.open_ix(&h, &ata, 0);
    bad.data.extend_from_slice(&[1, 2, 3]);
    assert!(w.env.send(&[bad], &[&holder]).is_err());
    assert_eq!(w.env.token_balance(&ata), STAKE * 2, "nothing moved");
}

#[test]
fn seat_limit_per_wallet() {
    let mut w = World::new();
    let (holder, ata) = w.holder(4);
    let h = holder.pubkey();
    for i in 0..3 {
        w.env.send(&[w.open_ix(&h, &ata, i)], &[&holder]).unwrap();
    }
    assert_eq!(
        w.env.send(&[w.open_ix(&h, &ata, 3)], &[&holder]).unwrap_err(),
        custom_err(0, SeatError::IndexOutOfRange as u32)
    );
}

#[test]
fn slash_blocks_withdraw_and_respects_timelock() {
    let mut w = World::new();
    let (holder, ata) = w.holder(1);
    let h = holder.pubkey();
    let seat = seat_pda(&h, 0);
    w.env.send(&[w.open_ix(&h, &ata, 0)], &[&holder]).unwrap();

    // только slash_authority
    let stranger = w.env.funded();
    assert_eq!(
        w.env.send(&[World::propose_slash_ix(&stranger.pubkey(), &seat, 500)], &[&stranger]).unwrap_err(),
        custom_err(0, SeatError::Unauthorized as u32)
    );
    // не больше max_slash_bps (10%)
    let slasher = w.slasher.insecure_clone();
    assert_eq!(
        w.env.send(&[World::propose_slash_ix(&slasher.pubkey(), &seat, 1_001)], &[&slasher]).unwrap_err(),
        custom_err(0, SeatError::SlashTooLarge as u32)
    );
    w.env.send(&[World::propose_slash_ix(&slasher.pubkey(), &seat, 500)], &[&slasher]).unwrap();

    // холдер пытается сбежать: разблокировка проходит, но вывод заблокирован висящим штрафом
    w.env.send(&[World::unlock_ix(&h, &seat)], &[&holder]).unwrap();
    w.env.warp_seconds(UNLOCK_DELAY);
    assert_eq!(
        w.env.send(&[w.withdraw_ix(&h, &seat, &ata)], &[&holder]).unwrap_err(),
        custom_err(0, SeatError::SlashPending as u32)
    );

    // таймлок штрафа (2 дня) уже прошёл вместе с 7 днями — исполняем
    w.env.send(&[w.execute_slash_ix(&seat)], &[]).unwrap();
    assert_eq!(w.env.token_balance(&w.treasury), STAKE / 20);
    assert_eq!(read_u64(&w.env.data(&seat), s::STAKED), STAKE - STAKE / 20);

    w.env.send(&[w.withdraw_ix(&h, &seat, &ata)], &[&holder]).unwrap();
    assert_eq!(w.env.token_balance(&ata), STAKE - STAKE / 20);
}

#[test]
fn slash_timelock_and_appeal() {
    let mut w = World::new();
    let (holder, ata) = w.holder(1);
    let h = holder.pubkey();
    let seat = seat_pda(&h, 0);
    w.env.send(&[w.open_ix(&h, &ata, 0)], &[&holder]).unwrap();
    let slasher = w.slasher.insecure_clone();

    w.env.send(&[World::propose_slash_ix(&slasher.pubkey(), &seat, 1_000)], &[&slasher]).unwrap();
    assert_eq!(
        w.env.send(&[w.execute_slash_ix(&seat)], &[]).unwrap_err(),
        custom_err(0, SeatError::TimelockNotElapsed as u32)
    );

    // апелляция удовлетворена
    let cancel = ixn(vec![ix::CANCEL_SLASH], vec![
        AccountMeta::new_readonly(slasher.pubkey(), true),
        AccountMeta::new_readonly(config_pda(), false),
        AccountMeta::new(seat, false),
    ]);
    w.env.send(&[cancel], &[&slasher]).unwrap();
    w.env.warp_seconds(TIMELOCK);
    assert_eq!(
        w.env.send(&[w.execute_slash_ix(&seat)], &[]).unwrap_err(),
        custom_err(0, SeatError::NoPendingChange as u32)
    );
    assert_eq!(w.env.token_balance(&vault_pda()), STAKE);

    // исполненный штраф переводит активное место в slashed и отвязывает устройство
    w.env.send(&[World::device_ix(ix::BIND_DEVICE, &h, &seat, [1u8; 32])], &[&holder]).unwrap();
    w.env.send(&[World::propose_slash_ix(&slasher.pubkey(), &seat, 100)], &[&slasher]).unwrap();
    w.env.warp_seconds(TIMELOCK);
    w.env.send(&[w.execute_slash_ix(&seat)], &[]).unwrap();
    let d = w.env.data(&seat);
    assert_eq!(d[s::STATUS], status::SLASHED);
    assert_eq!(&d[s::DEVICE_KEY..s::DEVICE_KEY + 32], &[0u8; 32]);
}

#[test]
fn config_change_waits_for_timelock() {
    let mut w = World::new();
    let admin = w.admin.insecure_clone();
    let new_slasher = Pubkey::new_unique();
    let mut data = vec![ix::PROPOSE_CONFIG];
    data.extend_from_slice(admin.pubkey().as_ref());
    data.extend_from_slice(new_slasher.as_ref());
    data.extend_from_slice(&(STAKE * 2).to_le_bytes());
    data.extend_from_slice(&UNLOCK_DELAY.to_le_bytes());
    data.push(2);
    let propose = ixn(data, vec![
        AccountMeta::new_readonly(admin.pubkey(), true),
        AccountMeta::new(config_pda(), false),
        AccountMeta::new_readonly(w.treasury, false),
    ]);
    let apply = ixn(vec![ix::APPLY_CONFIG], vec![AccountMeta::new(config_pda(), false)]);

    // не админ
    let stranger = w.env.funded();
    let mut bad = propose.clone();
    bad.accounts[0] = AccountMeta::new_readonly(stranger.pubkey(), true);
    assert_eq!(w.env.send(&[bad], &[&stranger]).unwrap_err(), custom_err(0, SeatError::Unauthorized as u32));

    w.env.send(&[propose], &[&admin]).unwrap();
    assert_eq!(w.env.send(&[apply.clone()], &[]).unwrap_err(), custom_err(0, SeatError::TimelockNotElapsed as u32));
    w.env.warp_seconds(TIMELOCK);
    w.env.send(&[apply], &[]).unwrap();
    let d = w.env.data(&config_pda());
    assert_eq!(read_u64(&d, c::STAKE_AMOUNT), STAKE * 2);
    assert_eq!(d[c::MAX_SEATS], 2);
    assert_eq!(&d[c::SLASH_AUTHORITY..c::SLASH_AUTHORITY + 32], new_slasher.as_ref());
    assert_eq!(d[c::HAS_PENDING], 0);
}

#[test]
fn set_authorities_is_immediate_and_only_admin() {
    let mut w = World::new();
    let admin = w.admin.insecure_clone();
    let new_admin = w.env.funded();
    let new_slasher = Pubkey::new_unique();
    let new_treasury = w.env.create_token_account(&w.mint, &new_admin.pubkey(), 0);
    let set = |signer: &Pubkey| {
        let mut data = vec![ix::SET_AUTHORITIES];
        data.extend_from_slice(new_admin.pubkey().as_ref());
        data.extend_from_slice(new_slasher.as_ref());
        ixn(data, vec![
            AccountMeta::new_readonly(*signer, true),
            AccountMeta::new(config_pda(), false),
            AccountMeta::new_readonly(new_treasury, false),
        ])
    };

    // не админ
    let stranger = w.env.funded();
    assert_eq!(w.env.send(&[set(&stranger.pubkey())], &[&stranger]).unwrap_err(), custom_err(0, SeatError::Unauthorized as u32));
    // казна другого токена
    let other_mint = w.env.create_mint(&admin.pubkey(), 6);
    let wrong = w.env.create_token_account(&other_mint, &admin.pubkey(), 0);
    let mut bad = set(&admin.pubkey());
    bad.accounts[2] = AccountMeta::new_readonly(wrong, false);
    assert_eq!(w.env.send(&[bad], &[&admin]).unwrap_err(), custom_err(0, SeatError::WrongTreasury as u32));

    // ожидающее предложение старого админа сбрасывается
    let mut data = vec![ix::PROPOSE_CONFIG];
    data.extend_from_slice(admin.pubkey().as_ref());
    data.extend_from_slice(w.slasher.pubkey().as_ref());
    data.extend_from_slice(&(STAKE * 2).to_le_bytes());
    data.extend_from_slice(&UNLOCK_DELAY.to_le_bytes());
    data.push(2);
    let propose = ixn(data, vec![AccountMeta::new_readonly(admin.pubkey(), true), AccountMeta::new(config_pda(), false), AccountMeta::new_readonly(w.treasury, false)]);
    w.env.send(&[propose], &[&admin]).unwrap();

    w.env.send(&[set(&admin.pubkey())], &[&admin]).unwrap();
    let d = w.env.data(&config_pda());
    assert_eq!(&d[c::ADMIN..c::ADMIN + 32], new_admin.pubkey().as_ref());
    assert_eq!(&d[c::SLASH_AUTHORITY..c::SLASH_AUTHORITY + 32], new_slasher.as_ref());
    assert_eq!(&d[c::TREASURY..c::TREASURY + 32], new_treasury.as_ref());
    assert_eq!(d[c::HAS_PENDING], 0);
    assert_eq!(read_u64(&d, c::STAKE_AMOUNT), STAKE);
    // старый админ больше не может
    assert_eq!(w.env.send(&[set(&admin.pubkey())], &[&admin]).unwrap_err(), custom_err(0, SeatError::Unauthorized as u32));
}

#[test]
fn set_mint_only_admin_and_only_with_empty_vault() {
    let mut w = World::new();
    let mint2 = w.env.create_mint(&w.admin.pubkey(), 9);
    let treasury2 = w.env.create_token_account(&mint2, &w.admin.pubkey(), 0);
    let vault2 = Pubkey::find_program_address(&[SEED_VAULT, mint2.as_ref()], &pid()).0;
    let set_ix = |signer: &Pubkey, stake: u64| {
        let mut data = vec![ix::SET_MINT];
        data.extend_from_slice(&stake.to_le_bytes());
        ixn(data, vec![
            AccountMeta::new(*signer, true),
            AccountMeta::new(config_pda(), false),
            AccountMeta::new_readonly(vault_pda(), false),
            AccountMeta::new(vault2, false),
            AccountMeta::new_readonly(mint2, false),
            AccountMeta::new_readonly(treasury2, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ])
    };

    // в хранилище есть залог — смена токена запрещена
    let (holder, ata) = w.holder(1);
    let h = holder.pubkey();
    w.env.send(&[w.open_ix(&h, &ata, 0)], &[&holder]).unwrap();
    let admin = w.admin.insecure_clone();
    assert_eq!(w.env.send(&[set_ix(&admin.pubkey(), 5)], &[&admin]).unwrap_err(), custom_err(0, SeatError::VaultNotEmpty as u32));

    // залог выведен — хранилище пустое; чужой подписант не может
    let seat = seat_pda(&h, 0);
    w.env.send(&[World::unlock_ix(&h, &seat)], &[&holder]).unwrap();
    w.env.warp_seconds(UNLOCK_DELAY);
    w.env.send(&[w.withdraw_ix(&h, &seat, &ata)], &[&holder]).unwrap();
    let stranger = w.env.funded();
    assert_eq!(w.env.send(&[set_ix(&stranger.pubkey(), 5)], &[&stranger]).unwrap_err(), custom_err(0, SeatError::Unauthorized as u32));
    assert_eq!(w.env.send(&[set_ix(&admin.pubkey(), 0)], &[&admin]).unwrap_err(), custom_err(0, SeatError::InvalidParams as u32));

    // админ меняет токен: новый mint, хранилище ["vault", mint], казна и залог
    w.env.send(&[set_ix(&admin.pubkey(), 5_000_000_000)], &[&admin]).unwrap();
    let d = w.env.data(&config_pda());
    assert_eq!(&d[c::MINT..c::MINT + 32], mint2.as_ref());
    assert_eq!(&d[c::VAULT..c::VAULT + 32], vault2.as_ref());
    assert_eq!(&d[c::TREASURY..c::TREASURY + 32], treasury2.as_ref());
    assert_eq!(read_u64(&d, c::STAKE_AMOUNT), 5_000_000_000);

    // место открывается уже новым токеном
    let k = w.env.funded();
    let ata2 = w.env.create_token_account(&mint2, &k.pubkey(), 5_000_000_000);
    let open2 = ixn(vec![ix::OPEN_SEAT, 0], vec![
        AccountMeta::new(k.pubkey(), true),
        AccountMeta::new(config_pda(), false),
        AccountMeta::new(seat_pda(&k.pubkey(), 0), false),
        AccountMeta::new(ata2, false),
        AccountMeta::new(vault2, false),
        AccountMeta::new_readonly(mint2, false),
        AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
    ]);
    w.env.send(&[open2], &[&k]).unwrap();
    assert_eq!(w.env.token_balance(&vault2), 5_000_000_000);
    // старым токеном — уже нельзя
    let (old, old_ata) = w.holder(1);
    assert!(w.env.send(&[w.open_ix(&old.pubkey(), &old_ata, 0)], &[&old]).is_err());
}

#[test]
fn set_mint_to_token_2022_then_stake_and_withdraw() {
    // главный токен с pump.fun (create_v2) — Token-2022
    let mut w = World::new();
    let mint2 = w.env.create_mint_2022(&w.admin.pubkey(), 6);
    let treasury2 = w.env.create_token_account_2022(&mint2, &w.admin.pubkey(), 0);
    let vault2 = Pubkey::find_program_address(&[SEED_VAULT, mint2.as_ref()], &pid()).0;
    let mut data = vec![ix::SET_MINT];
    data.extend_from_slice(&STAKE.to_le_bytes());
    let set = ixn(data, vec![
        AccountMeta::new(w.admin.pubkey(), true),
        AccountMeta::new(config_pda(), false),
        AccountMeta::new_readonly(vault_pda(), false),
        AccountMeta::new(vault2, false),
        AccountMeta::new_readonly(mint2, false),
        AccountMeta::new_readonly(treasury2, false),
        AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
    ]);
    let admin = w.admin.insecure_clone();
    w.env.send(&[set], &[&admin]).expect("set_mint 2022");

    let k = w.env.funded();
    let h = k.pubkey();
    let ata = w.env.create_token_account_2022(&mint2, &h, STAKE);
    let seat = seat_pda(&h, 0);
    let open = ixn(vec![ix::OPEN_SEAT, 0], vec![
        AccountMeta::new(h, true),
        AccountMeta::new(config_pda(), false),
        AccountMeta::new(seat, false),
        AccountMeta::new(ata, false),
        AccountMeta::new(vault2, false),
        AccountMeta::new_readonly(mint2, false),
        AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
    ]);
    w.env.send(&[open], &[&k]).expect("open_seat 2022");
    assert_eq!(w.env.token_balance(&vault2), STAKE);

    w.env.send(&[World::unlock_ix(&h, &seat)], &[&k]).unwrap();
    w.env.warp_seconds(UNLOCK_DELAY);
    let withdraw = ixn(vec![ix::WITHDRAW], vec![
        AccountMeta::new(h, true),
        AccountMeta::new(config_pda(), false),
        AccountMeta::new(seat, false),
        AccountMeta::new(ata, false),
        AccountMeta::new(vault2, false),
        AccountMeta::new_readonly(mint2, false),
        AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
    ]);
    w.env.send(&[withdraw], &[&k]).expect("withdraw 2022");
    assert_eq!(w.env.token_balance(&ata), STAKE);
    assert_eq!(w.env.token_balance(&vault2), 0);
}

#[test]
fn set_mint_rejects_unsafe_token_2022_extensions() {
    let mut w = World::new();
    let admin = w.admin.insecure_clone();
    // вечный делегат (12) мог бы вывести хранилище; метаданные (19, как у pump.fun) — допустимы
    for (ext, ok) in [(12u16, false), (1u16, false), (14u16, false), (19u16, true)] {
        let mint2 = w.env.create_mint_2022_with_ext(&admin.pubkey(), 6, ext, &[7u8; 32]);
        let treasury2 = w.env.create_token_account_2022(&mint2, &admin.pubkey(), 0);
        let vault2 = Pubkey::find_program_address(&[SEED_VAULT, mint2.as_ref()], &pid()).0;
        let mut data = vec![ix::SET_MINT];
        data.extend_from_slice(&STAKE.to_le_bytes());
        let cur_vault = Pubkey::new_from_array(w.env.data(&config_pda())[c::VAULT..c::VAULT + 32].try_into().unwrap());
        let set = ixn(data, vec![
            AccountMeta::new(admin.pubkey(), true),
            AccountMeta::new(config_pda(), false),
            AccountMeta::new_readonly(cur_vault, false),
            AccountMeta::new(vault2, false),
            AccountMeta::new_readonly(mint2, false),
            AccountMeta::new_readonly(treasury2, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
        ]);
        assert_eq!(w.env.send(&[set], &[&admin]).is_ok(), ok, "расширение {ext}");
    }
}

#[test]
fn network_seat_limit_500() {
    let mut w = World::new();
    let (holder, ata) = w.holder(2);
    let h = holder.pubkey();
    // в сети уже 499 мест: 500-е открывается, 501-е — SeatsFull; после вывода место снова свободно
    let cfg = config_pda();
    let mut acc = w.env.svm.get_account(&cfg).unwrap();
    acc.data[c::SEATS_OPEN..c::SEATS_OPEN + 8].copy_from_slice(&(c::MAX_TOTAL_SEATS - 1).to_le_bytes());
    w.env.svm.set_account(cfg, acc).unwrap();
    w.env.send(&[w.open_ix(&h, &ata, 0)], &[&holder]).unwrap();
    assert_eq!(read_u64(&w.env.data(&cfg), c::SEATS_OPEN), c::MAX_TOTAL_SEATS);
    let (other, other_ata) = w.holder(1);
    let o = other.pubkey();
    assert_eq!(w.env.send(&[w.open_ix(&o, &other_ata, 0)], &[&other]).unwrap_err(), custom_err(0, SeatError::SeatsFull as u32));
    assert_eq!(w.env.token_balance(&other_ata), STAKE, "nothing moved");
}
