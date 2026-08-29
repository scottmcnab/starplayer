# Reference — the original `STAR.EXE` user interface

Archaeology of `STARPLAY/STAR.ASM` (1,421 lines), the DOS front-end of StarPlayer 2.25.
This is the design reference for **A1 — the TUI homage**, and for what the telemetry API
must be able to feed.

Like `S3MLIB.ASM`, this source is **gutted**: only 16 support procedures survive
(`ParseCMDLine`, `ReadVariable`, `ShowCmdLineHelp`, `SetScreenSize`, `PrepareScreen`,
`BCDToDec`, `SubtractTime`, `WriteStr`, `_himemsize`, `_lomemsize`, `_gethimem`,
`_freehimem`, `Bin2Dec`, `EndScreen`, `DrawStarAnsi`, `DeCrunch`). The keyboard ISR,
popup menus, file browser, slot manager, channel/VU rendering and DOS shell are all
absent, surviving only as commented-out calls (`;call HookKeyboard`, `;call DrawVUBars`,
`;call EndScreen`).

**The entire data segment survived**, however — including every TheDraw-crunched screen
— so the visual design is fully recoverable and is reproduced below.

`STARPLAY/` is read-only. Never modify it.

---

## 1. Screen geometry

`SetScreenSize` sets BIOS mode `83h` (mode 3, no clear); `-z50` additionally issues
`INT 10h AX=1112h BX=0` (8×8 ROM font) for 80×50. `Last_Row` is 25 or 50; `Scr_Ypos` is
the last usable row (24 or 49) where the help bar lives.

Buffers, from DPMI extended memory:

| Buffer | Size | Purpose |
|---|---|---|
| `ScreenBuffer` | `160*50` — the comment reads `;160*(4+33)` | **4 header rows + 33 body rows** (1 column header + up to 32 channel rows) |
| `ScreenBuffer2/3/4` | `80*50*2` each | menu, file-menu and a third save-under buffer |
| `ExtraData` | `65+3` | scratch for menu text entry |
| `FileList` | `FileDataSize * 255` | one directory listing |
| `ListFileBuffer` | 8192 | playlist / listfile |
| `Song_Table` | `M_Struc_Size * 64` | **64 module slots** |

Everything is composed into `ScreenBuffer` then blitted with `rep movsd` to `0B8000h`.
`PrepareScreen` waits for vertical retrace (port `3DAh` bit 3) before drawing.

---

## 2. The main screen

Row 0 is `HeadLine`; rows 1–3 are `HeaderAnsi`:

```
    00000000001111111111222222222233333333334444444444555555555566666666667777777777
    01234567890123456789012345678901234567890123456789012345678901234567890123456789
 0 |░▒░ -+ starplayer +- protected mode multi-module player  (c) jedi / oxygen - ░▒░|
 1 | memory ┌ conventional:              title:                                     |
 2 |        │ extra:                   pattern:          spd:      volume:          |
 3 |        └ soundcard:                   row:          bpm:        song:          |
```

Row 4 is `InfoAnsi`, the channel-list column header:

```
    00000000001111111111222222222233333333334444444444555555555566666666667777777777
    01234567890123456789012345678901234567890123456789012345678901234567890123456789
 4 | ┌────── sample name ──────┐  #  vol. ┌─── vu bar ───┐ pan ┌─── current fx ───┐ |
```

Field boundaries: sample-name box cols **1–27** (25 chars inside), `#` at ~30, `vol.` at
33–36, VU-bar box cols **38–53**, `pan` at 55–57 (three chars — exactly the width of a
`Pan_Table` entry), current-fx box cols **59–78** (18 chars inside, which matches the
longest effect string).

Rows 5 .. 5+N−1 are one row per active channel, N = `_TotalChanNum`, max 32.
`PrepareScreen` blits the header row plus `TotalChanNum` more rows and stores the total
in `ExtraOffset`; when the song stops it blanks exactly that many rows again, then walks
downward blanking any leftover line whose attribute high nibble is 7. A cheap
dirty-rectangle scheme.

The bottom row (`Scr_Ypos`) is `HelpBar`:

```
    00000000001111111111222222222233333333334444444444555555555566666666667777777777
    01234567890123456789012345678901234567890123456789012345678901234567890123456789
   |   F10=menu  C=cd audio  D=dos shell  Z=toggle video  ?=help  ESC=quit to dos   |
```

### 2.1 Exact field positions

Two number printers: `@putdec` writes an 8-char field **right-justified**; `@putdec2` is
**left-justified**. All values use attribute `71h` — blue on light grey.

| Row | Col | Field | Source |
|---|---|---|---|
| 1 | 23 | conventional free bytes | `_lomemsize` (`INT 21h AH=48h BX=FFFF`, ×16) |
| 2 | 23 | extended free bytes | `_himemsize` (`INT 31h AX=0500h`) |
| 3 | 23 | soundcard RAM | `PM_GetDeviceRAM` |
| 1 | 44 | module title (29 chars) | `[__CurrentModule + _Title]` |
| 2 | 44 | `pattern:` as `pos+1` `/` `Ordnum` | `_MActualPos`, `_Ordnum` |
| 3 | 38 | `row:` | `_MActualRow` |
| 2 | 58 | `spd:` | `_MCurrentSpd` |
| 3 | 58 | `bpm:` | `_MCurrentBPM` |
| 2 | 71 | `volume:` | `PM_GetMasterVol` |
| 3 | 71 | `song:` | `CurrentSong` (slot 1–64) |

Note that "pattern" actually displays the **order-list position**, not the pattern
number — the source reads `_MActualPos` with a `;_MActualPatt` comment left behind. The
`_MActual*` snapshot set exists precisely so the display shows the row that is
*currently sounding* rather than the one being parsed.

### 2.2 The channel row

Three tables define what a channel row looks like. The drawing code was deleted; the
tables are intact.

```asm
Pan_Table       db      'LFT'
                db      ' 1 ',' 2 ',' 3 ',' 4 ',' 5 ',' 6 ',' 7 ',' 8 '
                db      ' 9 ',' A ',' B ',' C ',' D ',' E '
                db      'RGT'

NoteText        db      'C-','C#','D-','D#','E-','F-','F#','G-','G#'
                db      'A-','A#','B-'

VU_Colour_Table db      72h,72h,72h,72h,72h,72h,72h,72h     ; 8 green cells
                db      7eh,7eh,7eh,7eh,7eh,7eh             ; 6 yellow cells
                db      7ch,7ch                             ; 2 red cells
```

So: a **16-cell horizontal VU meter, green → yellow → red, on a light-grey background**.
The peak-hold value is `_VUBarLevel` in `ChannelData`, set to the channel volume on a new
note or a volume-column write, and **decayed by 2 per tick** in `__UpdateTracker`,
clamping at 0. Pan renders as `LFT` / `1`…`E` / `RGT` (16 S3M positions). Notes render as
`C-`, `C#`, … plus an octave digit.

### 2.3 Effects spelled out in English

The "current fx" column indexes two tables of human-readable names by command letter.
**This is the single most charming idea in the program** — instead of raw `A06` / `S8F`
hex like every other player, each active channel says what it is doing:

| Cmd | Text | Cmd | Text |
|---|---|---|---|
| `A` | `change speed` | `O` | `sample offset` |
| `B` | `jump to order` | `Q` | `note retrigger` |
| `C` | `break pattern` | `R` | `tremolo` |
| `D` | `volume slide` | `T` | `change tempo` |
| `E` | `slide down` | `U` | `fine vibrato` |
| `F` | `slide up` | `V` | `global volume` |
| `G` | `portamento` | `X` | `fine channel pan` |
| `H` | `vibrato` | | |
| `I` | `tremor` | `S1` | `glissando control` |
| `J` | `arpeggio` | `S2` | `set finetune` |
| `K` | `vibrato & vol. slide` | `S3` | `set vibrato waveform` |
| `L` | `porta & vol. slide` | `S4` | `set tremolo waveform` |
| | | `S8` | `channel pan` |
| | | `SB` | `pattern loop` |
| | | `SC` | `note cut` |
| | | `SD` | `note delay` |
| | | `SE` | `pattern delay` |

The per-channel data available for display comes from `ChannelData`: `_SampleNum`,
`_CurrentVol`, `_ActualVol`, `_CurrentNote`, `_TargetNote`, `_PanPosition`,
`_VUBarLevel`, `_CMDVal`, `_CMDData`, `_ActiveFlag`. The last four are explicitly marked
"for host program" / "info only" — **they exist purely to feed the UI**, which is exactly
the role of the modern telemetry API.

---

## 3. CD audio mode

Row 0 is the same `HeadLine`; rows 1–3 become `CDStatusLine`:

```
    00000000001111111111222222222233333333334444444444555555555566666666667777777777
    01234567890123456789012345678901234567890123456789012345678901234567890123456789
 1 | memory ┌ conventional:               · cd audio mode ·          volume:        |
 2 |        │ extra:                     track:         time:        remain:        |
 3 |        └ soundcard:                status:        total:        remain:        |
```

| Row | Col | Field |
|---|---|---|
| 1 | 73 | CD mixer volume (0–255) |
| 2 | 44 | `nn/nn` current track / highest audio track |
| 2 | 58 | `MM:SS` position within the track |
| 2 | 73 | `MM:SS` remaining in the track |
| 3 | 44 | status: `open` or `play` |
| 3 | 58 | `MM:SS` absolute disc time |
| 3 | 73 | `MM:SS` remaining on the disc |

A zero-padding trick worth copying: write a literal `'0'` at the cell, then skip one cell
if the value is ≥ 10 — giving `05:07`-style times.

`CDROMLIB.ASM` (812 lines) is the one **intact** file in the tree and holds the full
MSCDEX API: head position, get/set volume, UPC, audio info, set track, track length,
status, seek, play, stop, resume, eject/close/reset, position, media change, door
lock/unlock, plus RedBook↔HSG conversion.

---

## 4. Popup panels

**Slot / module menu** — `Menu_1` header + `Menu_2` repeated per slot + `Menu_3` footer:

```
   |   ╔════════════════════════════════════════════════════════════════════════╗   |
   |   ║  slot     ·∙· module  titles ·∙·       size    chan samps  len  patts  ║   |
   |   ║  ┌──┐ ┌────────────────────────────┐ ┌───────┐ ┌──┐ ┌───┐ ┌───┐ ┌───┐  ║   |
   |   ║  │  │ │                            │ │       │ │  │ │   │ │   │ │   │  ║   |  <- repeated
   |   ║  │  │ │                            │ │       │ │  │ │   │ │   │ │   │  ║   |
   |   ║  └──┘ └────────────────────────────┘ └───────┘ └──┘ └───┘ └───┘ └───┘  ║   |
   |   ╚════════════════════════════════════════════════════════════════════════╝   |
   | ┌────────────────────────────────────────────────────────────────────────────┐ |
   | │ L/TAB=load C=cdmode P=play S=stop V=view F=free R=release W=write ESC=quit │ |
   | └────────────────────────────────────────────────────────────────────────────┘ |
```

Columns: slot number, module title (28), size (7), channels (2), samples (3), length (3),
patterns (3). 64 slots; state in `Menu_TopSlot` / `Menu_Highlight` / `Menu_Size`.

**File selector** — `Filemenu_1` + `Filemenu_2` repeated + `Filemenu_3`:

```
   |   ╔════════════════════════════════════════════════════════════════════════╗   |
   |   ║       ┌────────────────────────────────────────────────────────┐       ║   |
   |   ║   path│                                                        │       ║   |
   |   ║       └────────────────────────────────────────────────────────┘       ║   |
   |   ║       filename      type       ·∙· module  titles ·∙·        size      ║   |
   |   ║   ┌──────────────┐  ┌───┐  ┌────────────────────────────┐  ┌───────┐   ║   |
   |   ║   │              │  │   │  │                            │  │       │   ║   |  <- repeated
   |   ╚════════════════════════════════════════════════════════════════════════╝   |
   |              ┌──────────────────────────────────────────────────┐              |
   |              │ SPACE=tag *=tag all ENTER=load/select ESC=cancel │              |
   |              └──────────────────────────────────────────────────┘              |
```

The browser **pre-scans each file and shows its module title**, plus type and size.

**Sample / instrument viewer** — `View_1` + `View_2` repeated + `View_3`:

```
   |   ╔════════════════════════════════════════════════════════════════════════╗   |
   |   ║  samp          instrument name           length    startlp   endloop   ║   |
   |   ║  ┌──┐  ┌─────────────────────────────┐  ┌───────┐ ┌───────┐ ┌───────┐  ║   |
   |   ║  │  │  │                             │  │       │ │       │ │       │  ║   |
   |   ╚════════════════════════════════════════════════════════════════════════╝   |
   |                        ┌──────────────────────────────┐                        |
   |                        │ UP/DOWN=slide list  ESC=quit │                        |
   |                        └──────────────────────────────┘                        |
```

**Filename entry box** — `FilePath`, white on light grey rather than the cyan of the
menus:

```
   |   ╔════════════════════════════════════════════════════════════════════════╗   |
   |   ║       ┌──────────────────────────────────────────────────────────────┐ ║   |
   |   ║ file: │                                                              │ ║   |
   |   ║       └──────────────────────────────────────────────────────────────┘ ║   |
   |   ╚════════════════════════════════════════════════════════════════════════╝   |
```

**CD audio keymap card** — `CD_Screen`, a 13-row two-column panel:

```
   |                           ┌───────────────────────┐                            |
   |                           │   · cd audio mode ·   │                            |
   |        ┌──────────────────┘                       └──────────────────┐         |
   |        │                              │                              │         |
   |        │      ESC = quit menu         │        F1-F8 = start track   │         |
   |        │                              │                              │         |
   |        │        C = quit cd audio     │      F11/F12 = back/forward  │         |
   |        │                              │                              │         |
   |        │       F9 = stop/resume track │ ctrl-F11/F12 = dec/inc track │         |
   |        │                              │                              │         |
   |        │        E = eject/close tray  │  alt-F11/F12 = dec/inc vol   │         |
   |        │                                                             │         |
   |        └─────────────────────────────────────────────────────────────┘         |
```

---

## 5. Colour scheme (CGA attribute nibbles)

| Element | bg | fg |
|---|---|---|
| Banner (row 0) | 1 blue | `-+ starplayer +-` 15 white; `protected mode ` 3 cyan; `multi-module ` 10 light green; `player  ` 3 cyan; `(c) jedi / oxygen - ` 15 white; `░▒░` end caps 1/0 |
| Header rows 1–3 | 7 light grey | labels 4 red, box rules 0 black, **values `71h` blue-on-light-grey** |
| Channel header | 7 | labels 4 red, box rules 0 black |
| Channel rows | 7 | VU cells from `VU_Colour_Table` |
| Help bar | 2 green | text 15 white, key names 14 yellow |
| Slot / file / view menus | 3 cyan | frame 11 light cyan + 8 dark grey (a 3-D bevel), headings 14 yellow, list interior bg 0 black |
| `FilePath` entry box | 7 light grey | frame 15 white, `file:` label 4 red |
| `CD_Screen` | 3 cyan | frame 11, key names 14 yellow |

---

## 6. Keyboard

From `Help_Msg1` / `Help_Msg2`, the `?` popup, verbatim:

```
starplayer quick help:
  command   - starplay [<filename>]
  hot keys  ┌ (with scroll lock on)
            │ f1-f8      play multiple songs continuous
            │ s-f1-f8    play one song looped
            │ f9         terminate song
            │ f10        popup song/file selection menu
            │ f11,f12    inc, dec pattern
            │ a-f11,f12  inc, dec volume
            └ c-f11,f12  inc, dec current song
```

| Context | Key | Action |
|---|---|---|
| Main | `F10` | popup menu |
| | `C` | CD audio mode |
| | `D` | DOS shell |
| | `Z` | toggle video (25 ↔ 50 rows) |
| | `?` | help |
| | `ESC` | quit to DOS |
| Slot menu | `L` / `TAB` | load |
| | `C` | cd mode |
| | `P` / `S` | play / stop |
| | `V` | view sample list |
| | `F` | free (drop the slot's sample RAM, keep the header) |
| | `R` | release (unload the slot entirely) |
| | `W` | write the playlist |
| | `ESC` | quit |
| File browser | `SPACE` / `*` | tag / tag all |
| | `ENTER` | load/select |
| | `ESC` | cancel |
| Sample viewer | `UP` / `DOWN` | scroll |
| | `ESC` | quit |
| CD mode | `F1`–`F8` | start track |
| | `F9` | stop/resume |
| | `E` | eject/close tray |
| | `F11` / `F12` | seek back/forward (10 s per step) |
| | `ctrl-F11/F12` | previous/next track |
| | `alt-F11/F12` | volume down/up |
| | `C` / `ESC` | quit cd audio / quit menu |

---

## 7. Command line

Verbatim from `Init_Msg0` + `CmdLne_Help1`:

```
-+ starplayer version 2.25 +-

usage:   starplay <-options> <filespec>

where options are:
         -d        disables DMA sample loading (gus only)
         -m######  sets mixing rate in hz, default is 44100 (sb only)
         -b######  sets buffer size in bytes, default is 1024 (sb only)
         -a###     forces a fixed amplification, default is 48 (sb only)
         -c#       select a soundcard, default is autodetect (GUS=1, SB=2)
         -z##      select screen size, 25 or 50 rows
<filespec> can be any file, or a wildcard for multiple files
```

Parser notes: `-` and `/` both accepted; case-folded; anything not starting with a switch
character becomes the filename. `-c` outside 1..2 falls through to the help. `-b` is
rounded down to a 16-byte multiple, 0 → 1024. `-a` is clamped to **16..127** despite the
help text saying "default is 48"; the in-memory default is actually 0. `-z` ≥ 50 selects
50-row mode.

Error strings:

```
ERROR: Not enough free extended memory.
ERROR: Could not initialise sound device.
Ensure the ULTRASND= or BLASTER= environment variables
are set correctly or try resetting your sound card (ie. ULTRINIT)
```

---

## 8. Background operation

Not a true TSR. It is a protected-mode PMODE/W program that stays resident in the
foreground, hooks the keyboard so it can pop up over whatever is on screen, and shells
out to `COMMAND.COM` so you can work while music plays.

- `ActiveFlag ;Set this when scrollock pressed` — **Scroll Lock is the hotkeys-armed
  gate**, matching the help text's `(with scroll lock on)`.
- `PopUpFlag ;Set this when in popup menu` — `PrepareScreen` skips channel-bar blitting
  entirely when set, so the status area freezes behind a menu.
- `KeyboardLockFlag`, `Old_Key_Handler`, `_OldRealMode`, `_RealModeCode db 21 dup(?)` —
  INT 9 hooked in **both** protected and real mode via the DPMI real-mode-callback
  pattern that `S3MLIB` uses for the sound IRQ.
- `ProtectFlag ;Flag to prevent irq from interrupting foreground music code` — a
  re-entrancy guard between the IRQ-driven player and the UI.
- DOS shell (`D`): `INT 21h AH=4Bh` on `COMSPEC`, with
  `ExitString db 'type exit to return to starplayer.$'`. The DTA is captured at startup
  so the file browser's FindFirst/FindNext can coexist with the shelled interpreter.
- `PM_SetLoopCode` installs a callback fired when the order list wraps — this is how
  "play multiple songs continuous" advances to the next slot from inside the player IRQ.

---

## 9. Files, slots and playlists

- **Command-line wildcard**: `<filespec>` can be a wildcard, scanned with
  FindFirst/FindNext.
- **Interactive browser**: 255 entries per directory, mixing directories, drive letters
  and modules, with a tag bit for multi-select and the module title pre-read from each
  file's header.
  ```asm
  FileData        struc
  _FileType       db      ?       ;1=s3m,2=mod,3=mtm,16=dir,32=drv,128=marked
  _FileName       db      13 dup(?)
  _FileTitle      db      28 dup(?)
  _FileSize       dd      ?
  FileData        ends
  ```
- **Playlist**: an 8 KB `ListFileBuffer`, written out by `W` in the slot menu.
- **Slots**: 64 module slots, each a full `Module` struct in extended memory.
  `CurrentSlot` (load target) and `CurrentSong` (playing) are separate.
- Supported types: `TypeTable db 'S3M',0,'MOD',0,'MTM',0`.

---

## 10. Other features worth carrying forward

- **Multi-module continuous play** (`F1`–`F8`) and **single-song loop**
  (`Shift-F1`–`F8`).
- **Seeking** by order position (`F11`/`F12`). No time-based scrub.
- **Song switching without stopping playback** (`ctrl-F11`/`ctrl-F12`).
- **No per-channel mute or solo** — nothing in the data supports it. Worth *adding* in
  the TUI, since the modern engine has it.
- **No fade.**
- **Live memory readout** — conventional and extended free memory plus soundcard RAM,
  refreshed every redraw. On a 1996 GUS this was genuinely useful: you could watch DRAM
  fill as you loaded slots. The modern equivalent is voice-pool occupancy and decoded
  PCM footprint.
- **`DeCrunch`** — a TheDraw "crunched screen" interpreter: bytes ≥ 32 are characters;
  0–15 set foreground; 16–23 set background; 24 = newline (resetting to the *starting*
  column, so panels are position-independent); 25 = a run of N+1 spaces; 26 = a run of
  N+1 of the next byte; 27 = toggle blink; 28–31 ignored. **Every panel is a tiny
  compiled bytecode blob rather than a layout routine.** If any single primitive is worth
  keeping in the TUI, it is this one.

---

## 11. The exit screen

`EndScreen` → `DrawStarAnsi` plus `ContactAnsi` at row 9:

```
 0 |█▓▒░        ▓░███       █████████▀▀▀▀▀ █ ▀▀▀▀▀▀▀ █████  ▀   ▀▀▀▀▀▀ ▀▀▀▀▀▀▀▀▀▀▒▓█|
 1 |█▒░      ▓▒░▒  █▓▒░ ░▒▓██▓▓█         ██ ██     ███   ███                     ░▒█|
 2 |█░     █▓▓▒▒             █▒▒█      ▒▒▓   ██   ███   ███                       ░█|
 3 |█    ▓███████████       ░░▒▒░░    ░▒█    ██▓█  ▓████▓       P L A Y E R        █|
 4 |█              ████     █░░▒▒   ░███▀▀▀   ▓███ ▓▓██ █▓▓                        █|
 5 |█  ░░▒▒▓▓██░░▒▒▓█████  ██▓▓░▒  ▒▓██      ░▒▓█   ▓▓██  ▒▒█                      █|
 6 |█ ░▒▓████░░▒▒▒▒▓▓██   ██▓▓▒▒░░  ▒▓██    ░▒▓█    ▒▒▓▓█  ▒░█░█                   █|
 7 |█  ░                            ░▒▓███        ░▒▒▓▓█     ░█░░▒▒▓                |
 8 |█▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄ ▄ ▄▄ ▄▄    ▄    ▄                ▒░▓█▓█▓▒▒░ ▄█ ▄ ▄▄▄▀|
 9 |█                                                                              █|
10 |█  contacting the author (jedi/oxygen) -                                       █|
```

The contact block below it carries 1996-era email, web and FTP addresses plus a postal
address. **Do not reproduce the personal contact details in the TUI homage** — the star
logo and the banner are the parts worth keeping.

---

## 12. What A1 should take from this

1. The **layout**: banner, three-row status header, per-channel rows with sample name,
   number, volume, VU, pan and effect, and a bottom help bar.
2. The **16-cell green/yellow/red VU** with peak-hold and a fixed decay per tick.
3. **Effects spelled out in English** — the signature feature.
4. The **`_MActual*` snapshot discipline**, so the display shows the row that is
   sounding, not the row being parsed.
5. The **panel aesthetic**: double-line outer frame, single-line field boxes, cyan
   panels with yellow headings.
6. The **star logo** on exit.

And what it should *add*, because the engine now supports it: per-channel mute and solo,
oscilloscopes, and a real pattern view rather than a single effect column.
