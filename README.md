<div align="center">
  <img src="./resources/assets/logo.svg" width="200" alt="GlazeWM logo" />

# GlazeWM (WSLg fork)

**A tiling window manager for Windows, with tiling support for WSLg windows.**

[About this fork](#about-this-fork) •
[Requirements](#requirements) •
[Installation](#installation) •
[Configuration](#configuration) •
[Troubleshooting](#troubleshooting) •
[Upstream docs](#upstream-documentation)

</div>

## About this fork

This is a fork of [glzr-io/glazewm](https://github.com/glzr-io/glazewm). It adds one thing: Linux GUI applications running under WSL2 tile properly instead of sitting letterboxed inside their own window frame.

Everything else is upstream GlazeWM. The fork tracks upstream `main` and carries the WSLg patch on top of it, so keybindings, config format, CLI, and IPC are unchanged. For anything not specific to WSLg, use the [upstream documentation](#upstream-documentation).

### The problem

WSLg publishes Linux GUI applications through RDP `RAIL_WINDOW` host windows. Those windows forward a resize to the Linux surface they host only while Windows considers them *arranged*, meaning snapped into a snap layout.

A tiling window manager positions windows with `SetWindowPos`, which does not arrange them. So stock GlazeWM moves and resizes the host window correctly, but the Linux application inside keeps its original size and ends up letterboxed.

### The workaround

When a `RAIL_WINDOW` is first managed, and again after each restore from a minimized state, GlazeWM briefly primes it. It injects a synthetic snap chord (`Win`+`Left` by default), waits for the OS to finish arranging the window, then re-applies the window's real tiling rect. After that, ordinary `SetWindowPos` calls reach the Linux surface and the window tiles like any other.

Priming only runs while the window has focus, and only once per window per shown or restored cycle.

This leans on OS behavior that Microsoft does not document and could change at any time, which is why it is not proposed upstream.

## Requirements

- Windows 10 or 11 with WSL2 and WSLg.
- Snap Assist turned off. This one is not optional, see below.

### Turn off Snap Assist

Priming abuses the OS snap mechanism, so Windows tries to follow the injected snap with its Snap Assist flyout, the panel offering to fill the other half of the screen. GlazeWM dismisses the flyout on its own, but that extra window stealing focus mid-arrangement makes priming slower and less reliable.

> Settings → System → Multitasking → expand **Snap windows** → turn off
> **"When I snap a window, suggest what I can snap next to it"**
> (German: *Beim Andocken eines Fensters anzeigen, was daneben angedockt werden kann*)

Leave the other snap options alone. Only this one needs to be off.

## Installation

Download the installer from this fork's [releases](https://github.com/bearylabs/glazewm/releases). Do not install from winget, Chocolatey, Scoop, or the upstream releases page, since none of those carry the WSLg patch.

The fork's builds are unsigned, so SmartScreen warns on first launch. Click "More info", then "Run anyway".

Unsigned also means UIAccess stays off, which upstream enables when it packages a signed build. Without UIAccess, GlazeWM cannot force the foreground window and cannot reposition windows of elevated processes. Everything else works the same.

Installing over an existing upstream GlazeWM is fine. The config file at `%userprofile%\.glzr\glazewm\config.yaml` works in both directions. The fork adds one optional key, and upstream GlazeWM ignores keys it does not know, so you can switch back without editing the config.

## Configuration

The WSLg workaround is on by default and lives under `general.snap_arrange`:

```yaml
general:
  snap_arrange:
    # Whether to snap these windows so that they can be resized.
    enabled: true

    # Modifier key of the snap chord to inject (e.g. 'lwin' for Win+Left).
    modifier_key: 'lwin'
```

Change `modifier_key` only if an input remapper such as PowerToys Keyboard Manager has remapped the left Windows key, or if one of your own keybindings would swallow the chord.

Set `enabled: false` to switch the workaround off. GlazeWM then treats WSLg windows like any other window, which puts you back to letterboxed Linux applications.

If your config predates the fork, you do not need to add anything. The defaults above apply when the key is missing.

## Troubleshooting

**A WSLg window stays letterboxed.** Priming only runs while the window has focus. Click the window, then move or resize it once. If it still does not resize, check that Snap Assist is off.

**A Snap Assist flyout appears when a WSLg window opens.** Snap Assist is still on. See [Turn off Snap Assist](#turn-off-snap-assist).

**A window flickers to half the screen and back when it opens.** That is priming. One flicker per window per shown or restored cycle is expected.

**The injected chord triggers one of my keybindings.** Pick a different `modifier_key`, or rebind the conflicting keybinding.

## Upstream documentation

The fork changes nothing outside WSLg handling, so upstream docs apply as written:

- [Default keybindings and command cheat sheet](https://github.com/glzr-io/glazewm#default-keybindings)
- [Config documentation](https://github.com/glzr-io/glazewm#config-documentation) (general, keybindings, gaps, workspaces, window rules, window effects, window behavior, binding modes)
- [FAQ](https://github.com/glzr-io/glazewm#faq)
- [Sample config](https://github.com/glzr-io/glazewm/blob/main/resources/assets/sample-config.yaml)
- [Building from source](https://github.com/glzr-io/glazewm/blob/main/CONTRIBUTING.md)
- [Zebar](https://github.com/glzr-io/zebar), the companion status bar

Bugs that reproduce on upstream GlazeWM belong in [upstream's issue tracker](https://github.com/glzr-io/glazewm/issues). Anything WSLg-related goes [here](https://github.com/bearylabs/glazewm/issues).

## Maintaining this fork

Notes for whoever keeps this thing current. Skip if you only want to use it.

### Repository layout

| Branch / remote     | Purpose                                                              |
| ------------------- | -------------------------------------------------------------------- |
| `main`              | Upstream `main` plus the WSLg patch. This is the branch to build.    |
| `origin` (remote)   | This fork.                                                            |
| `upstream` (remote) | [glzr-io/glazewm](https://github.com/glzr-io/glazewm), the original. |

The exact delta against upstream is always `git diff upstream/main main`.

### Syncing upstream

```sh
git fetch upstream
git checkout main
git merge upstream/main   # resolve conflicts, then build and test
git push origin main
```

Upstream is merged in, not rebased on top of, so the same conflicts do not have to be resolved twice.

This README replaces upstream's, so it conflicts whenever upstream edits theirs. Resolve it by keeping the fork's version:

```sh
git checkout --ours README.md
git add README.md
```

Nothing in the repository automates that, on purpose. A `merge=ours` entry in `.gitattributes` would work, but the merge driver it names has to be defined in local git config on every clone, and the point here is to keep the diff against upstream as small as possible.

### Releases

Releases come from `.github/workflows/release-fork.yaml`, triggered by hand from the Actions tab with a version number. It builds the Windows installers and opens a draft GitHub release. Review it, then publish.

Upstream's `release.yaml` cannot be used here. Its macOS job imports an Apple signing certificate from secrets that only the upstream repository holds. The fork workflow drops macOS packaging entirely, which is fine because the WSLg patch is Windows-only. Windows code signing is skipped too, hence the unsigned installers.

Version numbers are not generated anywhere. Whatever gets typed into the workflow ends up in the binary (`VERSION_NUMBER`), the installers, and the git tag. It is substituted into WiX's `Version` attribute, so it has to be purely numeric: `major.minor.patch` or `major.minor.patch.revision`. Suffixes like `3.9.1-wslg.1` break the installer build and the workflow rejects them up front.

The scheme is upstream version plus a fork revision, so `3.9.1.1` is the first fork release built on upstream `v3.9.1`.

### Contributing back to upstream

Changes meant for upstream must not branch off this fork's `main`, or the WSLg patch rides along in the diff. Branch off `upstream/main`:

```sh
git fetch upstream
git checkout -b fix/some-upstream-thing upstream/main
git push origin fix/some-upstream-thing
```

Then open the pull request against `glzr-io/glazewm:main`.

## License

Same as upstream, see [LICENSE.md](./LICENSE.md).
