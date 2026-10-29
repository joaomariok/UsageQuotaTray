# UsageQuotaTray

A Windows tray app that shows a user how much of their AI coding-assistant allowances and spend they have consumed, across any number of Assistants.

## Language

### Assistants and accounts

**Assistant**:
An AI coding product whose usage UsageQuotaTray monitors, such as Claude Code. The company behind it appears only in the Assistant's display name.
_Avoid_: service, provider, vendor, product, tool

**Account**:
One login to an Assistant, identified the way the Assistant's maker identifies it (for example, user plus organization). Where UsageQuotaTray found the login, such as a config folder, is not part of its identity.
_Avoid_: profile, user, config

**Source**:
One way of obtaining Meter values for an Account, such as a service endpoint, a hook the Assistant calls, or local files the Assistant writes.
_Avoid_: provider, adapter, feed, fetcher

### Meters

**Meter**:
One quantity shown to the user for an Account: how much of something has been used, optionally against a limit.
_Avoid_: gauge, stat, metric (reserved for OpenTelemetry metrics, which a Source may translate into Meters)

**Reading**:
One Meter's value at one moment, obtained from one Source, either Exact or Estimate.
_Avoid_: sample, data point, measurement

**Window**:
The span of time over which a Meter's usage accumulates before it resets, such as a 5-hour session, a week, or a billing month.
_Avoid_: period, cycle, interval

**Exact**:
A Meter value reported by the Assistant's maker.
_Avoid_: actual, precise, official

**Estimate**:
A Meter value UsageQuotaTray derives from partial data, such as locally recorded token counts priced with a published rate card.
_Avoid_: approximate, guess

### Tray icon

**Featured Meter**:
A Meter the user chose to show on the tray icon and in its tooltip.
_Avoid_: pinned meter (Windows uses "pin" for keeping a tray icon visible), primary meter

**Badge**:
The small mark on the tray icon that tells the user which Assistant the icon is showing.
_Avoid_: logo, glyph
