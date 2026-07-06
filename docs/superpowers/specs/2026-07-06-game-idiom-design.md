# Game interaction idiom (sub-project C) - design

Date: 2026-07-06.
Status: green-lit by Edd ("yes do this") after A and B merged to main (1c843c9).
Predecessors: `2026-07-06-conversational-captain-design.md` (A, merged), `2026-07-06-16bit-deck-design.md` (B, merged).
Visual reference (authoritative for all pixel values, chrome, and interaction feel): https://claude.ai/code/artifact/e0270ab6-290a-464c-93ac-76a8b9163ba8

## Summary

The deck is the stage; C makes every remaining surface speak its language.
The five stock egui windows become pixel-chrome station console screens.
The captain window becomes the mockup's RPG dialogue box, with portrait, typewriter text, a Mission Briefing card, and choice buttons.
Escalations and merge confirmations become hails in the same dialogue component.
The bottom composer disappears: all conversation happens inside the dialogue box, opened by clicking the captain's chair (or automatically when the ship hails you).
This completes the full 16-bit takeover: after C, no Apple-minimal chrome remains.

## Goals

- One dialogue component, reused three ways: Captain conference, Tactical escalation hail, Helm docking (merge) hail.
- Station console chrome for all detail windows: dark panel, chunky double border, pixel-font headers, tinted chips.
- Crisp text inside pixel chrome (the mockup's split): headers, labels, buttons, and chips use the pixel display font; body and log text stay comfortably readable.
- Typewriter text that respects `ui.reduce_motion` and completes on click.
- No new backend events or commands; C is presentation over the existing protocol.

## Non-goals

- Sound (still deferred).
- Deck scene changes (tactical officer sprite, lift-door slide, pip redesign stay on the pixel-pass list).
- Light mode: the deck is one committed visual world; C retires the light/dark split (see Theme).
- In-scene text rendering on the framebuffer (the DOM-crisp split stands).

## Fonts

Two OFL-licensed faces, committed under `assets/fonts/` with their license files, embedded via `egui::FontDefinitions` at startup:

- **Press Start 2P** (`PressStart2P-Regular.ttf`): display face for window headers, speaker names, chips, and buttons; used small (9 to 12 pt) and sparingly.
- **VT323** (`VT323-Regular.ttf`): body face for dialogue text, console readouts, and logs; used at 16 to 20 pt where it stays legible.

Existing proportional text (tooltips on the deck) may remain the default egui font; the fonts are additive families (`"pixel"`, `"crt"`), not a replacement of the default family.

## Theme

`theme.rs` is replaced by a single dark pixel theme (no `ThemeMode`, no light variant); `visuals()` commits egui to it regardless of OS theme.
Token values come from the mockup's page chrome:

- space `#060811` (window fill behind panels), panel `#0e1322`, panel-2 `#121933`, hairline `#232c47`, text `#c9d4e8`, dim text `#7c89a6`.
- Accent amber `#ffb648` (primary), alert red `#ff5a4e`, confirm green `#3fd68c`, console cyan `#59c8ff`.
- These deliberately match the deck's status constants in `sprites.rs` (single source where shared: the theme re-exports the sprite constants rather than redeclaring hex values).
- Corners are square (`CornerRadius::ZERO`) everywhere; no shadows.

## The console-screen chrome

A reusable `console_frame` renders the mockup's `#modal` treatment: fill `#080c18`, 2 px cyan border with a 3 px gap of window-fill then a 1 px outer hairline (the double-outline effect), square corners.
A `station_header` renders the title in Press Start 2P, cyan, uppercase, letter-spaced (e.g. `HELM CONSOLE - CAPTAIN-SESSION`, `TACTICAL - GUARDRAIL ADJUDICATIONS`, `KOBAYASHI MARU - BATTLE REPORT`, `SHIP'S COMPUTER - LOG`, `MISSION STATUS`).
A `chip` renders the mockup's chip: pixel font at 8 to 9 pt, 1 px border and 12 percent tinted fill in the semantic color (ALLOW green, DENY red, ESCALATE amber, info cyan, severity colors for findings).
All five station windows (workstream, tactical, kobayashi, computer, mission status) adopt this chrome; their information content is unchanged from today.
Buttons inside console screens use `pixel_button`: pixel font, filled accent background with a darker 2 px bottom edge (the mockup's pressed-shadow), square.

## The dialogue box

One component, the mockup's `#dlg`, rendered as a bottom-anchored egui area over the deck (inset 3 percent left/right, 3.5 percent bottom):

- Chrome: near-black fill at 96 percent opacity, 3 px light border (`#e8ecf5`) with the double-outline treatment, square corners.
- Left: a 3x-scaled pixel portrait (28x28 sprite map, NEAREST texture). The Captain's portrait is the mockup's `PORTRAIT` map, ported into `sprites.rs`. Tactical and Helm hails reuse the crew sprite palette for simple bust variants (back-view maps are not busts; C adds two small front-bust maps, gold uniform, drawn in the mockup's style).
- Header: speaker name in Press Start 2P, amber for the Captain, red for a Tactical priority hail, gold for Helm.
- Body: VT323 dialogue text with a typewriter reveal (about 2 characters per 16 ms tick, time-based); clicking the text completes it instantly; `ui.reduce_motion` renders it complete immediately; a blinking block cursor shows while revealing.
- Mission Briefing card (when a proposal is attached): amber-bordered inset titled `MISSION BRIEFING - PROPOSED PLAN` (or `REV N` / `PLAN AMENDMENT - DIFF vs CURRENT`), one row per workstream with the diff marker (`+` green for added, `~` amber for revised, `>` dim for unchanged) plus red `x slug (cancelled)` rows for removals.
- Choices: a row of pixel buttons. Captain conference: `MAKE IT SO` (green, sends `ApproveProposal`), plus free-text send. Escalation hail: `APPROVE ONCE`, `DENY` (red), `ALWAYS ALLOW` (sends the existing `UserDecision` variants). Merge hail: `ENGAGE` (green), `HOLD`.
- Input row (Captain conference only): a text field plus SEND pixel button inside the dialogue; Enter submits; sends `SayToCaptain`.

### Dialogue orchestration

- Clicking the captain's chair (`DeckAction::HailCaptain`) opens the Captain dialogue (replacing today's `captain_window`); it shows the transcript tail (last Captain message as the typewriter body; earlier history scrolls above it in a compact VT323 list) and the latest proposal card if one is pending.
- An incoming `EscalationRequested` opens the Tactical hail automatically (as the modal does today); `MergeConfirmationRequested` opens the Helm hail. Multiple pending items queue in the same order the current modals use.
- The dialogue closes on explicit close (Esc or the corner button) or when its purpose resolves (decision sent); the Captain dialogue stays open across turns while the user converses.
- The bottom composer is deleted; with no mission and no dialogue open, a small persistent `HAIL THE CAPTAIN` pixel button floats bottom-center as the entry point (the chair click does the same).

## Banners

The paused and compat banners restyle to slim pixel strips (amber and red tint, pixel-font label, VT323 detail) with unchanged behavior and actions.

## Testing

- Typewriter state machine is pure (chars-visible as a function of elapsed time, complete-on-click, reduce-motion) and unit-tested.
- Dialogue queueing (escalation before merge; captain dialogue independent) unit-tested at the state level.
- Chip/label mapping and theme token wiring unit-tested where pure.
- Font loading smoke-tested (FontDefinitions contains both families; startup does not panic without the assets being system-installed since they are embedded bytes).
- Visual quality lands in a human pixel pass at the end, alongside the outstanding B checklist items.

## Migration

C touches only `bridge-app` (plus committed font assets).
`theme.rs` shrinks to the pixel tokens; `ThemeMode` and the light palette are deleted; every `Tokens` consumer re-points to the new names (the compiler drives the sweep).
`bottom_composer`, `escalation_modal`, and `merge_modal` are deleted once their dialogue replacements land.
After C, the Apple-minimal idiom is fully retired.
