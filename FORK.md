# About this fork

This is a fork of [glzr-io/glazewm](https://github.com/glzr-io/glazewm) that adds tiling support for **WSLg windows**, the GUI windows that WSL2 projects onto the Windows desktop.

Everything else is unchanged. The fork tracks upstream `main` and carries the WSLg patch on top of it.

## The problem

WSLg publishes Linux GUI applications through RDP `RAIL_WINDOW` host windows. These windows only forward a resize to the Linux surface they host while Windows considers them *arranged*, that is, snapped into a snap layout.

A tiling window manager positions windows with `SetWindowPos`, which does not arrange them. The result: GlazeWM moves and resizes the host window correctly, but the Linux application inside keeps its original size, leaving it letterboxed inside its own frame.

## The workaround

When a `RAIL_WINDOW` is first managed, and again after each restore from a minimized state, GlazeWM briefly "primes" it: it injects a synthetic snap chord (`Win`+`Left` by default), waits for the OS to finish arranging the window, and then re-applies the window's real tiling rect. From that point on, ordinary `SetWindowPos` calls reach the Linux surface, and the window tiles like any other.

Priming happens only while the window has focus, and only once per window per shown/restored cycle.

This is a workaround built on OS behavior that Microsoft does not document and could change at any time, which is why it is not proposed upstream.

## Required Windows setting

Because the workaround abuses the OS snap mechanism, Windows will try to follow the injected snap with its **Snap Assist** flyout, the panel offering to fill the other half of the screen. GlazeWM dismisses the flyout automatically, but the extra window stealing focus mid-arrangement makes priming slower and less reliable.

**Disable Snap Assist:**

> Settings → System → Multitasking → expand **Snap windows** → turn off
> **"When I snap a window, suggest what I can snap next to it"**
> (German: *Beim Andocken eines Fensters anzeigen, was daneben angedockt werden kann*)

Leave the other snap options alone; only this one needs to be off.

## Configuration

The workaround is enabled by default and is configured under `general.snap_arrange`:

```yaml
general:
  snap_arrange:
    # Whether to snap these windows so that they can be resized.
    enabled: true

    # Modifier key of the snap chord to inject (e.g. 'lwin' for Win+Left).
    modifier_key: 'lwin'
```

`modifier_key` only needs to be changed if the left Windows key is remapped by an input remapper (e.g. PowerToys Keyboard Manager), or if one of your own keybindings would intercept the chord.

Set `enabled: false` to turn the workaround off entirely; GlazeWM then treats WSLg windows like any other window.

## Repository layout

| Branch / remote     | Purpose                                                             |
| ------------------- | ------------------------------------------------------------------- |
| `main`              | Upstream `main` plus the WSLg patch. This is the branch to build.   |
| `upstream` (remote) | [glzr-io/glazewm](https://github.com/glzr-io/glazewm), the original. |
| `origin` (remote)   | This fork.                                                          |

The exact delta against upstream is always `git diff upstream/main main`.

## Keeping the fork current

```sh
git fetch upstream
git checkout main
git merge upstream/main   # resolve conflicts, then build and test
git push origin main
```

Upstream changes are merged in (not rebased on top of), so the same conflicts do not have to be resolved again on every sync.

## Releases

Releases are built by the fork-only workflow `.github/workflows/release-fork.yaml`, triggered manually from the Actions tab with a version number. It produces the Windows installers and creates a **draft** GitHub release; review it, then publish.

Upstream's own `release.yaml` cannot be used here: its macOS job imports an Apple signing certificate from secrets that only the upstream repository holds. The fork workflow drops the macOS packaging entirely, which is fine because the WSLg patch only applies to Windows. Windows code signing is skipped as well, so the installers are unsigned and SmartScreen will warn on first launch.

Unsigned builds also mean UIAccess stays disabled (upstream enables it when packaging). UIAccess requires a signed executable in a secure location, and without it GlazeWM cannot force the foreground window or reposition windows of elevated processes. This matches how the fork is developed and tested locally, since `cargo build` leaves the `ui_access` feature off by default.

Version numbers are not generated anywhere; the value typed into the workflow is what ends up in the binary (`VERSION_NUMBER`), the installers, and the git tag. It is substituted into WiX's `Version` attribute, so it must be purely numeric: `major.minor.patch` or `major.minor.patch.revision`. Suffixes such as `3.9.1-wslg.1` break the installer build and are rejected by the workflow up front.

The scheme used here is **upstream version plus a fork revision**, so `3.9.1.1` is the first fork release built on upstream `v3.9.1`.

## Contributing back to upstream

Changes meant for upstream must not be based on this fork's `main`, since that would drag the WSLg patch and this file into the diff. Branch off `upstream/main` instead:

```sh
git fetch upstream
git checkout -b fix/some-upstream-thing upstream/main
git push origin fix/some-upstream-thing
```

Then open the pull request against `glzr-io/glazewm:main`.

## Building

See the upstream [contributing guide](https://github.com/glzr-io/glazewm/blob/main/CONTRIBUTING.md); the build is unchanged.
