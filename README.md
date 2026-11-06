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
