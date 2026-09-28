# DESIGN.md — NasoChat UI Design System

## Visual Theme & Atmosphere
High-performance developer tool aesthetic combining Geist dark mode precision with high-density terminal metrics.
- **Mantra:** Minimal precision. Design by subtraction. Every pixel serves performance telemetry or prompt interaction.
- **Canvas:** Ultra-dark monochrome surface (`#050505` / `#0A0A0B`). High contrast without visual noise.

## Color Tokens & Roles
- **Canvas Background:** `#050505` (Main surface)
- **Panel / Surface:** `#0F0F11` (Cards, sidebar, floating prompt bar)
- **Subtle Border:** `#27272A` (1px hairlines for clean separation)
- **Primary Text:** `#EDEDEF` (High emphasis)
- **Body / Muted Text:** `#A1A1AA` (Secondary labels, documentation)
- **Dimmed Text:** `#52525B` (Disabled, inactive states)
- **Accent Action / Active Status:** `#10B981` (Emerald green — active WebGPU node, streaming status, positive metrics)
- **Warning / VRAM Pressure:** `#F59E0B` (Amber — KV-cache allocation warnings)
- **Error:** `#EF4444` (Red — shader compilation or WASM fault)

## Typography & Scale
- **UI Font:** `Geist`, `Inter`, `-apple-system`, `sans-serif`
- **Code & Telemetry Font:** `Geist Mono`, `JetBrains Mono`, `Fira Code`, `monospace`
- **Rules:**
  - All metrics (VRAM, tokens/sec, TTFT, thread count) MUST use `font-mono`.
  - Body text uses 14px (`text-sm`) with `-0.01em` letter spacing.
  - Section headers use 500 weight. Never use heavy bold (>600) on dark surfaces.

## Component Geometry & Rules
- **Command Palette Input (`Cmd + K`):** Fixed bottom or center overlay with subtle outer hairline (`#27272A`), rounded `8px` (`rounded-lg`), ghost background (`#0F0F11`).
- **Telemetry Bar:** Top or bottom sticky header, 32px height, dense tabular layout with emerald status dot indicator.
- **Streaming Response Card:** Borderless message blocks with faint vertical hairline separator (`#27272A`).
- **Buttons:** Low-profile ghost buttons (`border border-zinc-800 hover:bg-zinc-900 text-zinc-200`).

## Do's and Don'ts
- **DO:** Use tabular numbers (`font-mono`) for rapidly changing streaming telemetry.
- **DO:** Keep borders at 1px hairline (`border-zinc-800`).
- **DON'T:** Use bright rainbow gradients, rounded pill buttons, or heavy drop shadows.
- **DON'T:** Use blue/purple consumer app accent colors; stick to monochrome + emerald status indicators.