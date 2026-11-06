# Setup Rush on-chain programs (Solana devnet)

The two Anchor programs behind [Setup Rush](https://setuprush.com), published so anyone can read them and check that the deployed bytes come from this source. Same license as the app: AGPL-3.0-only.

| Program | What it does | Devnet address |
|---|---|---|
| `rush_arena` | Live Match: escrowed game balances, capped play permissions, House-bot and friend rooms, settlement and refunds | [`5ZkKsZVj1ZxtiMnydGn82ojs4rSZ3ARpWLfP7w9AFYo9`](https://explorer.solana.com/address/5ZkKsZVj1ZxtiMnydGn82ojs4rSZ3ARpWLfP7w9AFYo9?cluster=devnet) |
| `setup_rush` | The Daily ("Clock In"): one shared chart a day, replay-verified scores, best score takes the pot | [`2jCQ5PCnNCPP4Kurf4Zz31YAzPzCk3zSfsDWd4eMZRgV`](https://explorer.solana.com/address/2jCQ5PCnNCPP4Kurf4Zz31YAzPzCk3zSfsDWd4eMZRgV?cluster=devnet) |

`rush_arena`'s interface (IDL) is published on chain through the program-metadata program, so Explorer lists its instructions and decodes its transactions.

## Check a build

```sh
solana-verify build --library-name rush_arena      # or setup_rush
solana-verify get-executable-hash target/deploy/rush_arena.so
solana-verify get-program-hash -u devnet 5ZkKsZVj1ZxtiMnydGn82ojs4rSZ3ARpWLfP7w9AFYo9
```

`release/` holds the reviewed `rush_arena` binary, its IDL and the rebuild evidence from its first devnet deploy.

## Verification status (6 November 2026)

Both programs are deployed from the verifiable build of commit `a8159f1` (`solana-verify build --library-name <name> --base-image solanafoundation/solana-verifiable-build:3.1.14 <absolute path to this repo>`):

| Program | On-chain hash = verifiable build | Upgrade |
|---|---|---|
| `rush_arena` | `7c85b66045024ad7616ab138252108f001d28036afaf4722d25fd932af925f28` | [2BfvxM47…](https://explorer.solana.com/tx/2BfvxM47DSAwi3GSWcLcDhxcgPYdm8oZgYhozh9EazSErim3H6MMvGqeTjo6RvQ4vbz1ntnwyWH1ad9iGeGVP6gi?cluster=devnet) |
| `setup_rush` | `5bbaba07ec6bade2dc951dda8a8858cf8b8d617268fb06debfd1d7eda26257aa` | [42jKPCGM…](https://explorer.solana.com/tx/42jKPCGM2iuPRTaqVQF5BSgeAW6LisEfFNsgs4vrCDr4FRULqLhWmFHkkCRFGcRnttbZKWZRU9c5c5n7MgoQeH5c?cluster=devnet) |

The verification records (repository, commit, library, image) are on chain for both, uploaded by the upgrade authority `7Jjt3omXwJznPYziXYjfWib218WKz2HQdptZD9u7LCFS`. OtterSec's devnet registry, which Explorer reads for its "Verified" badge on devnet, is offline today; the remote job runs once it returns. `release/` keeps the original October 12 reviewed binary for the record.
