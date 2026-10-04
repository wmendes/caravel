# Product

## Register

brand

The landing page (`site/`) and the README are the brand surfaces. The trading app has its own context in `lanes/perps/web/PRODUCT.md`.

## Users

Teams building an app on Stellar whose product needs rules the shared network can't give it. The rules can be a block time faster than Stellar's ~5 s ledgers, their own fees (or none), who may join, how many actions a user gets per block, which validators sign, or which token they settle in. Examples:
- a perps exchange that needs sub-second matching;
- a card program paying just in time from non-custodial wallets;
- a members-only payment network;
- a game economy;
- an agent marketplace.

They're developers and technical founders, comfortable with a CLI and a config file. Many come through the Stellar ecosystem (Meridian, hackathons, SDF programs). They read the page on a laptop, deciding in a minute whether this is worth an afternoon.

## Product Purpose

Caravel lets a team run its own appchain, a *lane*, with its own rules, while users' money stays on Stellar. One lane file declares the rules and every place the lane runs; `caravel plan` shows every change before `apply` makes it. Deposits, withdrawals and exits stay on Stellar: if the lane stops, anyone can freeze it and users take their balance back.

**The moat is owning the rules**, not any one of them. Speed (block time) is one rule among fees, access, limits, validators and the settlement token.

Caravel is open source (MIT or Apache-2.0), with no commercial offer today. Success: a visitor knows within the first screen who it's for and what it does, runs a lane on their laptop in about five minutes, and trusts the page because it says plainly what isn't done yet.

## Brand Personality

Precise, plain, honest. A harbor chart, not a rocket launch. It shows real runs and real numbers instead of adjectives. It never overstates: testnet only, not audited, validators run by the team for lane #1. The copy reads like a person wrote it: short sentences, few bullets, no hype words.

## Anti-references

- Crypto-launch pages: neon on black, gradient text, "the future of finance", token tickers, countdowns.
- Generic SaaS: hero metrics, identical icon-card grids, testimonial carousels, "trusted by" logo walls we don't have.
- The other IaC tool's branding or name (the copy rule); say "declarative" or "infrastructure as code".
- Claims we can't back: "trustless", "audited", "mainnet", "production-ready", "first", "validators on Stellar execute trades".

## Design Principles

1. **Who it's for, in the first screen.** The hero names the team and the job before it explains anything.
2. **Rules are the product.** Every example is a rule in the lane file, with the line that sets it.
3. **Show the run, not the claim.** Real outputs, real numbers, a live lane; each with its date or source.
4. **Honest, without the clutter.** The landing page stays short: it never claims more than is true, and says "testnet only, not audited" once, in the footer. The full list of edges (audit, mainnet, one operator for lane #1) lives in the README and the docs.
5. **One next step.** Run a lane in five minutes; everything else is secondary.

## Accessibility & Inclusion

WCAG 2.2 AA contrast in both themes, full keyboard use, visible focus. Reduced motion is respected: animations become static states. Live numbers never move layout (tabular figures, fixed widths). Color is never the only signal: coral = the lane and teal = Stellar always come with words.
