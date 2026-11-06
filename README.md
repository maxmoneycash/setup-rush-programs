# Setup Rush on-chain programs (Solana devnet)

The two Anchor programs behind [Setup Rush](https://setuprush.com), published so anyone can read them and check that the deployed bytes come from this source. Same license as the app: AGPL-3.0-only.

| Program | What it does | Devnet address |
|---|---|---|
| `rush_arena` | Live Match: escrowed game balances, capped play permissions, House-bot and friend rooms, settlement and refunds | [`5ZkKsZVj1ZxtiMnydGn82ojs4rSZ3ARpWLfP7w9AFYo9`](https://explorer.solana.com/address/5ZkKsZVj1ZxtiMnydGn82ojs4rSZ3ARpWLfP7w9AFYo9?cluster=devnet) |
| `setup_rush` | The Daily ("Clock In"): one shared chart a day, replay-verified scores, best score takes the pot less a fee of at most 5% | [`2jCQ5PCnNCPP4Kurf4Zz31YAzPzCk3zSfsDWd4eMZRgV`](https://explorer.solana.com/address/2jCQ5PCnNCPP4Kurf4Zz31YAzPzCk3zSfsDWd4eMZRgV?cluster=devnet) |

Both programs' interfaces (IDLs) are published on chain through the program-metadata program, so Explorer lists their instructions and decodes their transactions: `rush_arena`'s since 6 November 00:32 UTC, `setup_rush` v4's since 6 November 08:48 UTC ([IDL tab](https://explorer.solana.com/address/2jCQ5PCnNCPP4Kurf4Zz31YAzPzCk3zSfsDWd4eMZRgV/idl?cluster=devnet); the same JSON as `release/setup_rush.json`, built with `anchor idl build` from 2f04621).

## Check a build

```sh
solana-verify build --library-name rush_arena      # or setup_rush
solana-verify get-executable-hash target/deploy/rush_arena.so
solana-verify get-program-hash -u devnet 5ZkKsZVj1ZxtiMnydGn82ojs4rSZ3ARpWLfP7w9AFYo9
```

`release/` holds the reviewed `rush_arena` binary, its IDL and the rebuild evidence from its first devnet deploy, and `setup_rush`'s v4 IDL.

## Verification status (6 November 2026)

Both programs are deployed from the verifiable build (`solana-verify build --library-name <name> --base-image solanafoundation/solana-verifiable-build:3.1.14 <absolute path to this repo>`) of the commit in their row:

| Program | Commit | On-chain hash = verifiable build | Upgrade |
|---|---|---|---|
| `rush_arena` | `a8159f1` | `7c85b66045024ad7616ab138252108f001d28036afaf4722d25fd932af925f28` | [2BfvxM47…](https://explorer.solana.com/tx/2BfvxM47DSAwi3GSWcLcDhxcgPYdm8oZgYhozh9EazSErim3H6MMvGqeTjo6RvQ4vbz1ntnwyWH1ad9iGeGVP6gi?cluster=devnet) |
| `setup_rush` | `2f04621` (v4) | `a25d449b9de8d616cd341cc6e9373d46de079988fab4713c08c6f2d7d7e49cdf` | [3FA9bYom…](https://explorer.solana.com/tx/3FA9bYomw63ReX5yX925T3RZtfy7FedrberU4oxvCbZTgFXtd3kvBciWti1d44dEEz3Z9zsyxBWRgEHjXpQMb4kv?cluster=devnet) |

`setup_rush` v4 (6 November) adds the pot fee: a fee account set by the program's authority, at most 5%, that applies from the next UTC day's pools and is paid on every claim path; refunds pay none. The fee is 5% from the pools of 7 November ([set_fee](https://explorer.solana.com/tx/3NmZx2kyE1GvbX2ZfCFsb5KLGfZ71UhYafCUfxfjKQz4zW9FwRJv41qMpsCj2PrKGGu66iqtaHMUBkYDXwpuo8My?cluster=devnet)). v3 was the verifiable build of `a8159f1`, hash `5bbaba07ec6bade2dc951dda8a8858cf8b8d617268fb06debfd1d7eda26257aa`.

The verification records (repository, commit, library, image) are on chain for both, uploaded by the upgrade authority `7Jjt3omXwJznPYziXYjfWib218WKz2HQdptZD9u7LCFS`. OtterSec's devnet registry, which Explorer reads for its "Verified" badge on devnet, is offline today; the remote job runs once it returns. `release/` keeps the original October 12 reviewed binary for the record.
