//! Ничего не делает и всегда успешна. Только для тестов.

#[cfg(target_os = "solana")]
pinocchio::entrypoint!(process_instruction);

pub fn process_instruction(
    _program_id: &pinocchio::Address,
    _accounts: &mut [pinocchio::AccountView],
    _data: &[u8],
) -> pinocchio::ProgramResult {
    Ok(())
}
