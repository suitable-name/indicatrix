# 19. The Command Palette and the Keyboard

## What you will do

Indicatrix Cut has a lot of actions, and only a few of them have a key of their own.
The **command palette** is one small window that lists all of them. You type a few
letters of what you want, press **Enter**, and it happens: no hunting through menus,
tabs and panels, and no mouse needed. This chapter covers how to open it, how the
search behaves, what the rows tell you, which actions it offers, and where it stops.
The shortcuts that do have their own key are collected at the end and in Appendix B.

## Opening the palette

There are three ways, and they all do the same thing:

- Press **Ctrl+K**.
- Press **Ctrl+Shift+P**.
- Choose **Edit → Command Palette...** in the menu bar.

The two key presses work **anywhere in the window, even while you are typing in a text
field**, because neither of them types a character. The menu item is greyed out only
while one of the confirmation dialogs (unsaved changes, overwrite, close) or the
start-up restore question is on screen: answer that first.

The palette opens in the middle of the window with the cursor already in its search
box, so you can start typing at once.

## Searching

Type a few letters of the action's name. The list narrows as you type and the best
match is highlighted at the top.

- **The search is forgiving.** The letters only have to appear in order, not next to
  each other. `sv` finds **Save**, `dsolve` finds **Deep Solve**, and an
  abbreviation such as `exgcs` finds **Export as Gem Cut Studio (.gcs)...**
- **Capitals do not matter.**
- **The start of a word counts for more** than a letter in the middle of one, so `od`
  ranks a title with words starting O and D above a title that merely contains an o
  followed later by a d.
- **Every word you type has to match.** `export png` finds only commands that mention
  both. This is the quickest way to narrow a long family such as the export commands.
- **A match in the title beats a match in the hidden search words.** Each command also
  carries a few alternative words, so `revert` finds Undo, and `documentation` finds
  the User Manual. The category (File, Edit, View and so
  on) is searched too, so typing `view` lists the view commands.
- A shorter title beats a longer one when the match is otherwise equal: **Save** is
  listed above **Save As...** when you type `save`.
- When two commands match equally well, they keep the same order as in the list below,
  so the result of a search never jumps around.

If nothing matches, the palette says so and shows no rows. Clear a few letters and the
list returns.

## Moving around and choosing

| Key | What it does |
| --- | --- |
| Up / Down | Move the highlight one row. At the end of the list it wraps round to the other end. |
| Page Up / Page Down | Move the highlight a page (eleven rows) at a time, without wrapping. |
| Home / End | Jump to the first / last row. |
| Enter | Run the highlighted command. |
| Esc | Close the palette without doing anything. |
| Ctrl+K or Ctrl+Shift+P | Close the palette again (the keys open and close it). |

Pointing at a row with the mouse highlights it, and clicking a row runs it. Clicking
outside the palette's card closes it. Tab does nothing in the palette: the search box
is the only place that takes typing, so you cannot lose your place, and the rows are
chosen with Up and Down, the way a drop-down list is. Every row tells a screen reader
its name, its category, its shortcut and, when it is dimmed, why; hover a row for the
same words in a tooltip.

The list scrolls to keep the highlighted row visible, and shows at most twelve rows at
a time. The bottom line of the card reminds you of the keys and says how many commands
match.

## Recent commands

Before you type anything, the palette shows the commands you ran lately first, newest
at the top (up to eight), followed by everything else, grouped by category and sorted
by name. If you do the same few things all day, such as Solve and Save, they are
already at the top when you press Ctrl+K.

This memory lives only as long as the app is running. It is not saved, so each start
begins with the plain category order. Cancelling out of the palette does not count as
running a command, and neither does choosing a command that turns out to be
unavailable.

## Why some rows look dim

A row that cannot run right now is not hidden: it is shown dimmed with a short reason
under its name. That way you can see that the command exists and what it is waiting
for. Examples:

| Reason | Meaning |
| --- | --- |
| Open or create a design first | The command works on the design in the editor, and there is none yet. |
| Select a tier first | The command needs a selected tier (for example Duplicate Selected Tier). |
| Nothing to undo / Nothing to redo | The history is empty in that direction. |
| Wait for the running solve to finish | A solve, Deep Solve or Optimize is running, and this command would change the design underneath it. |
| No solve is running | Abandon Solve and the Cancel commands only make sense while their job runs. |
| Take a snapshot first | Compare to Snapshot needs a snapshot (Chapter 14). |
| Needs a library design with printed proportions | Deep Solve compares against catalogue proportions (Chapter 8). |
| Select a design in the library first | Load Selected Library Design needs a highlighted design. |
| Open the Edit tab first | The command only makes sense in the Edit tab. |
| Not available in the Diagram view | Slice mode needs the 3D picture, not the flat diagram. |
| Already using the Simple interface | You are already in the interface the command would switch to. |
| Switch to the Advanced interface first | The control exists only in the Advanced interface (Inspector: Schedule Tab; Chapter 17). |
| Locked by the guide | A guide or tutorial step has locked this control (Chapters 7 and 22). |

Picking a dimmed command does not close the palette. It shows a short message instead,
such as "Save is not available right now. Open or create a design first." so you learn
why, and you can go on typing to pick something else.

The palette follows exactly the same rules as the buttons and menus. It never lets you
do something the matching button would refuse; this includes the guide's locks, which
apply to the palette as strictly as they apply to the controls themselves.

## What happens when a command runs

The palette closes first, and the command starts a fraction of a second later. This is
deliberate: a command that opens a dialog (New Design..., Preferences..., Retarget...)
would otherwise lose the keyboard focus to the palette's closing. After the command has
run, keyboard shortcuts such as F5 and Ctrl+Z work again at once, without clicking
into the window first.

## What the palette offers

The commands are grouped in seven categories, each shown as a small label on the right
of the row.

- **File:** New Design..., Open..., Save, Save As..., Save as Variant... (Chapter 18),
  Load Selected Library Design,
  Export Edited .asc, Export as Gem Cut Studio (.gcs)..., Export Cutting Sheet
  (HTML)..., Export Diagram (PNG)..., Show Last Saved File in Folder.
- **Edit:** Undo, Redo, Edit Instructions as Text... (Chapter 11), Preferences..., Use the
  Simple Interface, Use the Advanced Interface.
- **View:** Go to 3D Spectral Preview, Go to Cutting Instructions, Go to Files &
  Downloads, Show Live Render, Show Edit Tab, the four Solid viewport modes (Solid,
  Path-Traced, Solid and Path-Traced, Diagram), the two Live Render modes, the
  inspector tabs (Tier, Preform, Optimize, Schedule, History) and the History tab's
  Variants view (Chapter 18), Show or Hide the Library Panel,
  Toggle Slice Mode, Toggle Snap to Gear Steps.
- **Tiers:** Add Tier, Add Concave Tier, Duplicate Selected Tier, Delete Selected Tier,
  Move Selected Tier Earlier / Later, Adopt All Imported Meets, Adopt Imported Meets of
  Selected Tiers, Filter Tiers, Clear Tier Selection.
- **Solve:** Solve, Abandon Solve, Deep Solve, Cancel Deep Solve, Optimize, Cancel
  Optimize, Retarget..., Snapshot Design, Compare to Snapshot, Compute Tilt Curves, and
  the five Auto-solve settings (Off, 150 ms, 300 ms, 1 s, 3 s).
- **Tools:** Plan Rough..., Angle Sweep... (Chapter 21), Cutting Mode... (Chapter 20), the
  three Regenerate commands for the catalogue's preview images and tilt curves, Show Tilt
  Performance Chart, New or Edit Gem Material..., Render Settings..., Export Render Image...
- **Help:** User Manual, Help for This Screen (F1), Keyboard Shortcuts, Guide: New Design
  Walkthrough, Tutorials..., Build the Selected Library Design and Welcome Tour
  (Chapter 22).

Every command that has a key of its own shows that key at the right-hand end of its
row, so the palette also teaches you the shortcuts: look at what you ran, and use the
key next time.

Where a command is a pair of states (Simple and Advanced interface, Slice mode, Snap,
the Library panel), the palette flips the same setting the matching button flips, so
the button and the palette never disagree.

## The shortcuts worth learning

You never need to learn any of these, because the palette finds everything. The ones
below are the ones people use all day. Appendix B lists every one, grouped by where it
works, and the in-app **Help → Keyboard Shortcuts** window (or **?**) shows the same
list.

| Area | Keys |
| --- | --- |
| File | Ctrl+N New, Ctrl+O Open, Ctrl+S Save, Ctrl+Shift+S Save As |
| History | Ctrl+Z Undo, Ctrl+Y (or Ctrl+Shift+Z) Redo |
| Solving | F5 Solve |
| Tabs | Ctrl+1, Ctrl+2, Ctrl+3 switch the three top-level tabs; Ctrl+E toggles Live Render and Edit |
| Solid viewport | 1, 2, 3, 4 switch the view mode; S toggles Slice mode |
| Tier list | Up / Down, Home / End, Delete, Ctrl+D, Alt+Up / Alt+Down, F2 |
| Search | Ctrl+F focuses the search or tier filter |
| Everywhere | Ctrl+K opens the palette, Ctrl+, opens Preferences, Esc backs out, ? shows the shortcuts |

A key that types a character (a digit, S, ? or a comma) does nothing while you are
typing in a text field, so you can still type those characters. That is another reason
to keep Ctrl+K in mind: it works even then.

## Limits of the palette

- It lists **actions**, not data. It does not search your library designs, your tiers
  or your files by name; the catalogue's search box and the tier list's filter do
  that. **Filter Tiers** (Ctrl+F in the Edit tab) is the palette's way to jump to the
  filter.
- There is no **Open Recent** list in it, and the import commands are not in it yet;
  use the menus and panels they live in for those.
- It does not press the buttons inside a dialog. Open the dialog from the palette, then
  use its own fields and buttons.
- While a confirmation question (unsaved changes, overwrite, close) is on screen,
  the menu item is greyed out. Answer the question before you start anything else.
- The recent-commands list is forgotten when you close the app.

## Troubleshooting

| Symptom | Likely cause | What to do |
| --- | --- | --- |
| Ctrl+K does nothing | A dialog or another window has the keyboard, so the main window never sees the key | Click the main window (or close the other dialog) and press Ctrl+K again; or use Edit → Command Palette.... |
| A command is dim although it should work | One of its conditions is not met | Read the reason under its name; it names the missing condition. |
| A key such as S or 1 does nothing | A text field has the focus | Click an empty part of the window, or press Esc, then press the key again. |
| The palette does not list a command you remember | It is a button inside a dialog, or an import | See "Limits of the palette" above. |
