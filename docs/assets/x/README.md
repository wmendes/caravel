# Caravel on X

Everything for the account, in the landing page's colors, font and mark. Each image is rendered from the `.html` next to it; edit that and render again.

## Images

| File | Use | Size |
|---|---|---|
| `avatar-800.png` | Profile picture (upload this one; X shows 400×400, cropped to a circle) | 800×800 |
| `avatar-400.png` | The same at X's display size | 400×400 |
| `header-3000x1000.png` | Header (upload this one; sharper on high-density screens) | 3000×1000 |
| `header-1500x500.png` | The same at X's recommended size | 1500×500 |
| `post-1600x900.png` | Image for the pinned post (16:9, shown uncropped in the feed) | 1600×900 |

The header and the profile picture don't depend on each other's position, so they work on the web and on phones alike. The header's left third stays empty (the avatar's corner on every layout), and its text and chart sit in the middle band, which phones never crop. It matches the avatar through shape, not position: the mark's coral sails become the lane's blocks, and its teal hull becomes Stellar's ledgers, the last one with the hull's slanted end.

Render again after an edit (from the repository root):

```sh
C="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"; D=docs/assets/x
r() { "$C" --headless=new --hide-scrollbars --force-device-scale-factor=$4 --window-size=$2,$3 --screenshot="$PWD/$D/$5" "file://$PWD/$D/$1"; }
r avatar.html 400 400 2 avatar-800.png;  r avatar.html 400 400 1 avatar-400.png
r header.html 1500 500 2 header-3000x1000.png;  r header.html 1500 500 1 header-1500x500.png
r post.html 1600 900 1 post-1600x900.png
```

## Profile

- **Name** (11/50): `Run Caravel`
- **Handle:** `@runcaravel`
- **Bio** (152/160):

  > For Stellar apps that need their own rules. Run your app as a lane: your block time, fees, members and validators, in one file. Open source, on testnet.

- **Website:** `https://caravel-tau.vercel.app`
- **Location:** `Lisbon` (the site's footer says "Built in Lisbon")
- **Category** (if you switch to a professional account): Software

## Pinned post

263/280 characters (X counts a link as 23). Attach `post-1600x900.png`.

> Caravel is open source, for Stellar apps that need their own rules.
>
> Your app runs as a lane: its own block time, fees, members and validators, declared in one file. Your users' money stays on Stellar.
>
> Run one on your laptop in 5 minutes: https://caravel-tau.vercel.app

## Alt text

- **Profile picture:** Caravel's mark: four coral bars stacked like sails over a teal hull.
- **Header:** "For Stellar apps that need their own rules. Your block time, fees, members and validators. Your users' money stays on Stellar." Next to it, coral lane blocks run above teal Stellar ledgers, with checkpoints dropping between them.
- **Post image:** "For Stellar apps that need their own rules." Next to it is an example lane file, with a block every 0.5 s, members only, zero maker fees, two-of-three validators and settlement in USDC, and the line "Your users' money stays on Stellar."

## Copy rules

The site's rules apply: no em dashes, nothing we can't back ("trustless", "audited", "mainnet", "first"), and the other IaC tool is never named. Say "testnet" wherever a post could read as live money.
