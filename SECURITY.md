# Security policy

## Reporting a vulnerability

Please do **not** open a public issue for security problems.

Report privately through [GitHub Security Advisories](../../security/advisories/new)
or a direct message to [@MyrmexSI](https://x.com/MyrmexSI) on X.

Include what is affected (program, instruction, package), how to reproduce it, and the impact you expect.
We will confirm receipt, investigate and keep you updated until a fix is deployed.

## Scope

- The on-chain programs in `programs/` deployed on Solana mainnet
- The protocol, SDK and worker in `packages/` and `apps/worker`

The worker runs untrusted job content: order text is data, never instructions. Reports about sandbox escapes,
key exposure or signature forgery are especially welcome.
