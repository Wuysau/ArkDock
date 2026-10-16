---
name: ArkDock
description: A deep-navy glass instrument that berths beside your app window and carries a real HarmonyOS preview.
colors:
  bg: "#0b1220"
  surface: "#111c2d"
  surface-elevated: "#152238"
  panel: "#17243a"
  surface-overlay: "rgb(17 28 45 / 94%)"
  text: "#eaf1fb"
  muted: "#a5b4c9"
  accent: "#4b82ff"
  accent-soft: "#78b8ff"
  highlight: "#78b8ff"
  action: "#356eea"
  action-hover: "#245bd2"
  selected: "rgb(75 130 255 / 14%)"
  success: "#39cf84"
  warning: "#f5b84b"
  danger: "#ff687a"
  line: "rgb(192 211 240 / 13%)"
  line-soft: "rgb(192 211 240 / 10%)"
  line-strong: "rgb(192 211 240 / 24%)"
typography:
  display:
    fontFamily: '"Segoe UI", "Microsoft YaHei UI", sans-serif'
    fontSize: "21px"
    fontWeight: 650
    lineHeight: 1.5
    letterSpacing: "-0.5px"
  headline:
    fontFamily: '"Segoe UI", "Microsoft YaHei UI", sans-serif'
    fontSize: "19px"
    fontWeight: 650
  title:
    fontFamily: '"Segoe UI", "Microsoft YaHei UI", sans-serif'
    fontSize: "15px"
    fontWeight: 600
    lineHeight: 1.6
  body:
    fontFamily: '"Segoe UI", "Microsoft YaHei UI", sans-serif'
    fontSize: "12px"
    lineHeight: 1.75
  label:
    fontFamily: '"Segoe UI", "Microsoft YaHei UI", sans-serif'
    fontSize: "11px"
  code:
    fontFamily: '"Cascadia Code", Consolas, monospace'
    fontSize: "12px"
    lineHeight: 1.7
rounded:
  button: "8px"
  card: "12px"
  shell: "16px"
  device: "20px"
spacing:
  space-1: "4px"
  space-2: "8px"
  space-3: "12px"
  space-4: "16px"
  space-6: "24px"
components:
  button:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text}"
    rounded: "{rounded.button}"
    padding: "7px 11px"
    height: "32px"
  button-hover:
    backgroundColor: "{colors.selected}"
    textColor: "{colors.highlight}"
  button-primary:
    backgroundColor: "{colors.action}"
    textColor: "#ffffff"
    rounded: "{rounded.button}"
    padding: "9px 14px"
    height: "36px"
  button-primary-hover:
    backgroundColor: "{colors.action-hover}"
  icon-button:
    backgroundColor: "transparent"
    textColor: "{colors.muted}"
    rounded: "{rounded.button}"
    size: "32px"
  input:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.text}"
    rounded: "{rounded.button}"
    padding: "10px 12px"
  chip:
    backgroundColor: "{colors.selected}"
    textColor: "{colors.highlight}"
    rounded: "5px"
    padding: "3px 5px"
  card:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text}"
    rounded: "{rounded.card}"
    padding: "16px"
  sheet:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.text}"
    rounded: "{rounded.shell}"
    width: "100%"
  theme-card:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.text}"
    rounded: "10px"
    padding: "10px 10px 30px"
  status-pill:
    backgroundColor: "{colors.surface-overlay}"
    textColor: "{colors.muted}"
    rounded: "{rounded.button}"
    height: "28px"
    padding: "4px 9px"
---

# Design System: ArkDock

> **Source of truth.** `src/styles.css` owns the token values and `docs/design/arkdock-ui.md` owns the
> product and acceptance rules; this file and `.impeccable/design.json` are derived specs. Change the
> CSS first, then update both here, so the normative frontmatter above cannot drift from the shipped UI.

## Overview

**Creative North Star: "The Dockside Instrument"**

ArkDock reads like a precision instrument berthed beside the workbench. The shell is deep-harbor navy glass — `#0b1220` at night, tonal slates stepping up to the panel — separated by hairlines of tinted light, with a single beacon-blue family marking whatever is live, selected, or actionable. Nothing decorative is allowed to pretend to be informative: a dot sits next to a word, a hairline divides sections, a glow appears only when a real frame arrives. The instrument reports; it does not perform.

The instrument is small on purpose. It lives in a 360–560 DIP column beside a window it never touches, so density is a feature: a 12px working band of type, a 4/8/12/16/24 spacing rhythm, one column of stacked surfaces, and the native frame claiming the flexible middle. Controls behave the way good hardware does — visible state, immediate confirmation, no surprise. The five themes are skins over one mechanism, not five different instruments.

Motion confirms events rather than asking for attention: a 140ms state tint, a 230ms sheet rise, a 600ms dock ripple after a confirmed attachment — plus a first-class reduced-motion ladder (off / light / standard / enhanced on top of system settings). The reference scene set the tone; its Agent window, desktop chrome, promotional annotations, and example bookkeeping content stay outside the product surface.

**Key Characteristics:**
- Deep-harbor navy glass with translucent hairlines and a 1px lit rim on every card-like surface
- One beacon-blue signal family; color expresses state, never paint
- A single narrow column where the native preview owns the flexible space
- Status as dot + label pairs at 10–11px — readable at a glance, never color alone
- Five swappable themes over one token contract; the classic dark skin is normative in this file
- Motion that confirms real events only, with reduce-motion and four intensity levels

## Colors

A harbor at night: layered navy surfaces, one beacon-blue signal, and three status colors that stay in their lanes.

### Primary

- **Beacon Blue** (#4B82FF): The single accent. Full-strength for borders, focus visuals, live dots, and the brand mark's base; ambient tints derive from it (Beacon Tint at 14% for selection, ripples at 42% stroke). It never fills a large area at full strength.
- **Ice Highlight** (#78B8FF): Interactive text and icons — paths, links, active glyphs, hovered labels. In the classic dark skin it equals Beacon Glow (#78B8FF), the soft tone for decorative strokes (scanner rings, orb symbol, welcome art); the light skins separate the two roles.
- **Command Blue** (#356EEA): The primary CTA fill behind a subtle vertical gradient (`#4279F0` → `#356EEA`). Reserved for the one action a surface most wants (信任此工程, 应用布局).
- **Command Blue Deep** (#245BD2): The pressed/hover state of the primary action; also the slider thumb's border tone.

### Status

- **Verified Green** (#39CF84): Confirmed and live states — bound target, running preview, passing diagnostics. The live dot adds a 14% halo.
- **Amber Caution** (#F5B84B): Warnings, stale frames, blocked builds, degraded trust.
- **Coral Alarm** (#FF687A): Errors and the destructive affordance only (the close button hovers to a 15% coral wash); failed diagnostics recolor text, never the whole row.

### Neutral

- **Deep Harbor Navy** (#0B1220): The window canvas and the base of the shell gradient.
- **Hull Slate** (#111C2D): Resting surface for buttons and settings cards.
- **Deck Slate** (#152238): Elevated fields — search inputs, directory input, list rows.
- **Bridge Indigo** (#17243A): The panel and sheet surface that rises above content.
- **Overlay Glass** (`rgb(17 28 45 / 94%)`): Chrome triggers and toasts; blurs at 12px where the platform supports it, opaque otherwise.
- **Frost White** (#EAF1FB): Primary text.
- **Harbor Mist** (#A5B4C9): Secondary text, captions, placeholders, disabled labels.
- **Hairlines** (`rgb(192 211 240 / 10% · 13% · 24%)`): Soft / default / strong borders, drawn in tinted light rather than gray.

### Named Rules

**The One Beacon Rule.** Blue is a signal, not a paint. Full-strength beacon blues mark what is live, selected, actionable, or focused; ambient presence (card washes, glows, ripples) stays at ≤16% alpha. If everything glows, nothing is a signal.

**The Five Skins, One Instrument Rule.** Five themes (system / light / dark / minimal-light / titanium-dark; system resolves to the classic light-dark pair) swap the same token names and nothing else. Layout, type, motion, and component structure never change with a theme, and no surface may introduce a color that is not a token.

**The Translucent Hairline Rule.** Borders are tinted light with alpha (or a tinted solid in light skins), never neutral gray. In the dark skins the figure is light-on-dark at 10/13/24%; in light skins the same structure is navy at comparable alpha.

## Typography

**Display Font:** "Segoe UI" with "Microsoft YaHei UI" (sans-serif fallback)
**Body Font:** the same system stack — Chinese-first rendering without a webfont download
**Label/Mono Font:** "Cascadia Code" with Consolas

**Character:** A native system stack, deliberately: a companion tool must boot instantly and render zh-CN copy with first-class metrics. The system's voice is exact, not loud — weight 650 appears only at display sizes, while 10–12px working text carries the density. Monospace is a first-class citizen: every path, ID, timestamp, and diagnostic value is set in the mono stack.

### Hierarchy

- **Display** (650, 21px, line-height 1.5, letter-spacing −0.5px): The empty-state headline only ("拖到开发窗口旁开始").
- **Headline** (650, 19px): Sheet titles (设置, 项目候选).
- **Title** (600, 15px, line-height 1.6): Section headings inside panels and cards.
- **Body** (400, 12px, line-height 1.75): Paragraphs and settings copy; dense rows may drop to 11px.
- **Label** (400–600, 11px): Status lines, metadata, buttons. The eyebrow variant is 10px with +1px tracking, used as "ARKDOCK" above panel titles.
- **Mono** (12px, line-height 1.7; 11px inside panels): Paths, diagnostic messages, log codes, measured values, with tabular figures for numbers.

### Named Rules

**The 12px Anchor Rule.** The document base is 12px and the working band is 10–12px; anything above 15px is reserved for display and headline moments, and body text never scales below 10px. If a control needs more room, it gets more space, not smaller type than the anchor.

## Layout

The window is a fixed-geometry column: 440×820 DIP by default, width locked to 360–560 DIP and height floored at 640 DIP by the native shell. Inside, one vertical stack: a 56px header, the workspace strip, the preview stage (the only flexible region), and a status footer; edge overlays (toolbar trigger on top, status trigger on bottom) float on the stage so chrome never competes with the frame. Sheets and toasts rise from the bottom inside the window; the diagnostics sheet scrolls independently.

The spacing rhythm is 4 / 8 / 12 / 16 / 24px. Window padding sits at 10–12px; cards keep 6–10px side margins; panels pad 16px. Compression is explicit rather than fluid: at ≤390px width, paddings drop to 8–9px, labels shrink, the window's minimize/maximize pair hides and the 收起 label collapses to the orb icon; at ≤690px height the welcome art scales to 0.7–0.85 and margins tighten; at ≤560px height the diagnostics sheet switches to the 8px rhythm. The stage keeps its share at every size — horizontal action rows wrap rather than shrink.

### Named Rules

**The One-Column Rule.** One column, one job. Nothing may add a second navigational column or a control panel that takes vertical space from the native frame; new controls first live in edge overlays, sheets, or collapse into triggers.

## Elevation & Depth

Depth is ambient, never theatrical. Surfaces are separated in three layers — tonal step, hairline, then a soft diffuse shadow — and every card-like surface carries a 1px inset top light that reads as a lit rim. Shadows are always wide, soft, and low-contrast (16–28% black, 16–36px blur). Glass is real but conservative: the header blurs at 12px and sheet backdrops at 4px where the platform supports backdrop-filter, with opaque fallbacks everywhere. The device bezel is the single "physical" object in the system and carries the strongest stack.

### Shadow Vocabulary

- **Lit rim** (`inset 0 1px 0 rgb(255 255 255 / 4%)`): every card, button, candidate, and theme card at rest — the signature top light.
- **Card rest** (`0 4px 16px rgb(0 0 0 / 16%)`): workspace strip, settings cards, evidence blocks.
- **Floating** (`0 6px 18px rgb(0 0 0 / 24%)`): chrome triggers, toasts, status pills, art highlights.
- **Sheet lift** (`0 12px 36px rgb(0 0 0 / 28%)`): bottom sheets and the inspector's selection panel.
- **Device frame** (`0 5px 18px rgb(0 0 0 / 22%), inset 0 0 0 1px rgb(126 145 168 / 13%)`): the phone bezel — inset ring plus drop, the deepest object allowed.

### Named Rules

**The Lit-Rim Rule.** Every card-like surface carries the 1px inset top light; a bright top edge is how this system says "resting surface". Cards without the rim look unfinished.

**The Ambient-Only Rule.** Shadows are wide, soft, and low-contrast; never tight dark drop shadows, never more than one stacking step below the device frame. Elevation reads from tonal steps and hairlines first, shadow second.

## Shapes

Corners are softly curved and disciplined: 8px for buttons, inputs, and triggers; 12px for cards; 16px for sheets; 20px for the device bezel (a 15px inner surface). Chips and evidence tags sit at 4–5px; icon tiles at 6–7px (the 44px section icon is the one exception at 12px); circles exist only for dots, swatches, orb affordances, and spinner rings — there are no pill-shaped buttons and no frosted cut-corners. Borders are 1px everywhere except the device shell, which steps to 2px in the light and titanium profiles for a machined edge. The application shell itself is square and brimless: the native window draws the only outer boundary, and painting a CSS radius there would create a second, inner arc.

### Named Rules

**The Native-Edge Rule.** ArkDock never draws its own outer frame. The operating system owns the window edge; decorative roundness belongs to objects inside it (device bezel, sheets, cards). Concretely: `.app` stays `border-radius: 0`.

## Components

Controls behave like physical instrumentation: crisp edges, visible state, no surprise.

### Buttons

- **Shape:** 8px radius (`--radius-button`), 1px hairline border, inline-flex with an 8px icon gap; min-height 32px.
- **Secondary (default):** Hull Slate fill with Frost White text; hover shifts to the Beacon Tint wash with a Beacon Blue border; active mixes 23% accent into the surface; disabled drops to 62% opacity with muted text.
- **Primary:** Command Blue gradient with white text, 36px min-height, lit rim; hover flattens to Command Blue Deep. At most one per surface — it marks the action the screen most wants (信任此工程, 应用布局).
- **Icon buttons:** 32×32 transparent, Harbor Mist glyphs; they inherit the standard hover wash. The window-close button is the only coral affordance (15% wash on hover).
- **Text buttons:** transparent, Ice Highlight text at 11px, used for tertiary reads ("了解拖动与工程识别").

### Chips and Badges

- **Badge:** 5px radius, Beacon Tint wash, Ice Highlight text at 10px (e.g. 可执行, evidence badges); invalid variants recolor to Harbor Mist.
- **Evidence tags:** 4px radius, hairline border, Harbor Mist text at 10px.
- **State chips** (pass / fail / blocked): recolor text only — Verified Green / Coral Alarm / Amber Caution — keeping the neutral chip shell.

### Cards and Containers

- **Corner Style:** 12px (`--radius-card`); 1px hairline border; lit rim; card shadow.
- **Background:** Hull Slate, or the card gradient (`linear-gradient(135deg, rgb(141 181 249 / 4%), transparent)` over the surface) for the visually raised workspace strip.
- **Workspace card:** a 46px compact row — 26–30px icon tile (7px radius, Beacon Tint), project name at 12px over a dot + status line at 10px, and an expand chevron that rotates 90° while the details open on a grid-rows animation.
- **Candidate cards:** full-width buttons; the reliable candidate is pre-lit (Beacon Blue border + Beacon Tint fill), the reason line separated by a hairline.
- **Theme cards:** two-up grid; a swatch strip (canvas / surface / accent), heading with icon, selected state = Beacon Blue border plus check dot with a 2px ring; description reserved along the bottom edge.

### Inputs and Fields

- **Search field:** strong hairline (24%) on Deck Slate, 8px radius, 10×12px padding; the magnifier sits in Harbor Mist; focus keeps the global 2px focus ring at 2px offset.
- **Directory input:** mono stack, 9–10px padding, flexible width down to 180px.
- **Range slider (Orb transparency):** a 4px track that fills Beacon Blue up to the thumb; a 16px thumb painted with the primary gradient, scaling to 1.1 on hover and 1.18 with an accent halo while dragging; the range caps at 80% transparency by product rule, because the orb is the recovery affordance.
- **Checkboxes / switches:** the platform control with `accent-color: Beacon Blue`.

### Navigation

- **Header:** 56px; the draggable zone carries the brand mark (two rotated 8×26px rounded bars in blue gradients, `#3F69FF→#42C3FA` and `#5B93FF→#2874F0`), a status line (dot + label, 11px) that turns Verified Green when bound, and 32px window actions.
- **Menu rows:** full-width, icon + label + trailing chevron, transparent borders at rest.
- **Segmented toggles** (interaction mode, motion preference): selected = Beacon Tint fill, Beacon Blue border, Ice Highlight text/icon.

### Bottom Sheet

The signature modal surface, drawn inside the window rather than as a separate one. A 45% navy backdrop (blurred 4px where supported) fades in, and the sheet rises 16px over 230ms. It opens with a 32×3px grab handle; the title block pairs the 10px "ARKDOCK" eyebrow with a 19px headline and a 32px close button; the body scrolls independently with a thin scrollbar. Sheets cap at `calc(100% - 44px)` height and stay above 160px.

### Preview Chrome

The stage's second signature: edge-revealed glass instrumentation that never blocks the frame. The top "预览控制" trigger and the bottom status trigger are 28px glass pills (`surface-overlay`, floating shadow) that reveal their bodies within an 180ms translate on hover, focus, or press; trigger chevrons rotate ±90° when open. Bodies are glass cards with 12px radius that pad 40px on the trigger side so the pill stays clickable. Pointer events are limited to the triggers and open bodies; the chrome never renders into the target content.

### Status Footer

The instrument's readout: a 52px full-width summary row — a 7px dot (with a 3px success halo when live), the preview status at 12px, a 10px meta line (frame generation and time, or build id and duration), a diagnostic-count chip (Beacon Tint, 10px), and a log icon. Error and warning recolor the dot and text only. The compact variant (dot + label pill) is the only status shown while the window is collapsed to the orb.

### Empty State

The first-run signature: a schematic pair — a rotated window frame and a device outline joined by a link badge — drawn with accent washes at 18–52%, replaced by a scanner orbit (dashed + solid rings, 2200ms rotation) while discovery actually runs. The welcome state shows exactly one primary action plus three read-only capability lines; the CTA is the only primary button on screen.

## Do's and Don'ts

### Do:

- **Do** express every color through a token; the frontmatter of this file is normative for the classic dark skin and every theme swaps the same names.
- **Do** keep ambient blue at ≤16% alpha and reserve full-strength beacon for live, selected, and actionable states.
- **Do** pair state with a dot + label at 10–11px; status is a first-class citizen and never color alone.
- **Do** set every path, ID, timestamp, and diagnostic value in the mono stack (11–12px) with tabular figures.
- **Do** keep UI responses at 140–230ms and reserve 600ms for the dock ripple; honor reduce-motion (1ms) and the off / light / standard / enhanced intensity ladder.
- **Do** keep the native frame on the stage: new controls live in 28px triggers, overlays, and sheets first.
- **Do** respect the 12px anchor when adding controls; if a control needs room, give it space — body text never scales below 10px.

### Don't:

- **Don't** paint a CSS border-radius or ring on the app shell; the native window owns the outer edge and a second arc would show between the two curves.
- **Don't** introduce hues outside the token set, and keep status colors single-purpose: Verified Green and Coral Alarm are signals, not decoration.
- **Don't** draw fabricated progress, invented percentages, or fake paths; the discovery orbit spins only while a real task runs and disappears the moment it ends.
- **Don't** let the promotional scene leak into the product: the wave-and-mesh glow, the reference Agent window, desktop chrome, and the sample bookkeeping content are context, not UI vocabulary.
- **Don't** use the shell theme to imitate the HarmonyOS app's own appearance; themes change ArkDock's shell only.
- **Don't** load web fonts or icon packages; icons are inline 24-box strokes at 1.65px with round caps (the drag grip thickens to 3.5px), in `currentColor`.
- **Don't** use pill-shaped buttons or tight dark drop shadows; softness in this system comes from alpha and blur, not from smaller radii or heavier ink.
