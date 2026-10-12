//! Регистрация агента на место, смена URI, закрытие после закрытия места.

use agent_registry::{AgentError, agent, config, ix, seat_ref};
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use swarm_test_utils::*;

fn pid() -> Pubkey {
    Pubkey::new_from_array(agent_registry::ID.to_bytes())
}
fn seat_program() -> Pubkey {
    Pubkey::new_from_array(seat_ref::PROGRAM.to_bytes())
}
fn cfg() -> Pubkey {
    Pubkey::find_program_address(&[config::SEED], &pid()).0
}
fn agent_pda(seat: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[agent::SEED, seat.as_ref()], &pid()).0
}

/// Поддельное место seat_registry с нужным владельцем и статусом (раскладка как у программы).
fn put_seat(env: &mut Env, owner: &Pubkey, status: u8) -> Pubkey {
    let key = Pubkey::new_unique();
    let mut data = vec![0u8; seat_ref::LEN];
    data[0] = seat_ref::DISC;
    data[seat_ref::STATUS] = status;
    data[seat_ref::OWNER..seat_ref::OWNER + 32].copy_from_slice(owner.as_ref());
    let lamports = env.svm.minimum_balance_for_rent_exemption(data.len());
    env.svm
        .set_account(key, Account { lamports: lamports, data: data, owner: seat_program(), executable: false, rent_epoch: 0 })
        .unwrap();
    key
}


fn uri_data(disc: u8, uri: &str) -> Vec<u8> {
    let mut d = vec![disc];
    d.extend_from_slice(&(uri.len() as u32).to_le_bytes());
    d.extend_from_slice(uri.as_bytes());
    d
}

#[test]
fn register_update_close() {
    let mut env = Env::new(&[(pid(), "agent_registry")]);
    let admin = env.funded();
    let init = Instruction {
        program_id: pid(),
        accounts: vec![
            AccountMeta::new(admin.pubkey(), true),
            AccountMeta::new(cfg(), false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data: vec![ix::INITIALIZE],
    };
    env.send(&[init], &[&admin]).unwrap();

    let holder = env.funded();
    let seat = put_seat(&mut env, &holder.pubkey(), 0);
    let register = |uri: &str, owner: &Pubkey, seat: &Pubkey| Instruction {
        program_id: pid(),
        accounts: vec![
            AccountMeta::new(*owner, true),
            AccountMeta::new(cfg(), false),
            AccountMeta::new_readonly(*seat, false),
            AccountMeta::new(agent_pda(seat), false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data: uri_data(ix::REGISTER, uri),
    };

    // чужое место не зарегистрировать
    let stranger = env.funded();
    assert_eq!(
        env.send(&[register("https://x/a.json", &stranger.pubkey(), &seat)], &[&stranger]).unwrap_err(),
        custom_err(0, AgentError::NotSeatOwner as u32)
    );
    env.send(&[register("https://api.example/agents/by-token/1.json", &holder.pubkey(), &seat)], &[&holder]).unwrap();
    let d = env.data(&agent_pda(&seat));
    assert_eq!(read_u64(&d, agent::AGENT_ID), 1);
    let n = read_u16(&d, agent::URI_LEN) as usize;
    assert_eq!(&d[agent::URI..agent::URI + n], b"https://api.example/agents/by-token/1.json");

    // второе место получает следующий id
    let seat2 = put_seat(&mut env, &holder.pubkey(), 0);
    env.send(&[register("ipfs://cid", &holder.pubkey(), &seat2)], &[&holder]).unwrap();
    assert_eq!(read_u64(&env.data(&agent_pda(&seat2)), agent::AGENT_ID), 2);

    // неактивное место не регистрирует
    let seat3 = put_seat(&mut env, &holder.pubkey(), 1);
    assert_eq!(
        env.send(&[register("ipfs://cid", &holder.pubkey(), &seat3)], &[&holder]).unwrap_err(),
        custom_err(0, AgentError::SeatInactive as u32)
    );

    let update = Instruction {
        program_id: pid(),
        accounts: vec![
            AccountMeta::new_readonly(holder.pubkey(), true),
            AccountMeta::new_readonly(seat, false),
            AccountMeta::new(agent_pda(&seat), false),
        ],
        data: uri_data(ix::UPDATE_URI, "ipfs://new"),
    };
    env.send(&[update], &[&holder]).unwrap();
    let d = env.data(&agent_pda(&seat));
    assert_eq!(read_u16(&d, agent::URI_LEN), 10);
    assert_eq!(&d[agent::URI..agent::URI + 11], b"ipfs://new\0");

    let close = Instruction {
        program_id: pid(),
        accounts: vec![
            AccountMeta::new(agent_pda(&seat), false),
            AccountMeta::new_readonly(seat, false),
            AccountMeta::new(holder.pubkey(), false),
        ],
        data: vec![ix::CLOSE],
    };
    assert_eq!(env.send(&[close.clone()], &[]).unwrap_err(), custom_err(0, AgentError::SeatStillOpen as u32));
    // место закрыто в seat_registry — запись агента закрывается, рента владельцу
    env.svm.set_account(seat, Account { lamports: 0, data: vec![], owner: SYSTEM_PROGRAM_ID, executable: false, rent_epoch: 0 }).unwrap();
    env.send(&[close], &[]).unwrap();
    assert!(!env.exists(&agent_pda(&seat)));
}
