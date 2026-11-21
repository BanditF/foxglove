# Testing MSI install, upgrade, downgrade, uninstall and autostart without touching the user's PC

Research only (11/20). Nothing was enabled, installed or changed to write this. The question: how
can agents test the Hideout/Foxglove MSI (per-machine install, upgrade, downgrade, uninstall) and
autostart moves (Startup-folder shortcut, Run key) without using the user's real Windows.

## Facts about this PC

- Windows 11 Pro 10.0.26300, 63 GB RAM, `HypervisorPresent = True`.
- `WindowsSandbox.exe` is absent, so the Sandbox feature is not enabled.
- Optional-feature state (`Get-WindowsOptionalFeature`) needs elevation to read; I did not
  elevate, so whether Hyper-V's management parts are enabled is unconfirmed. A running hypervisor
  means virtualization is on in firmware and something (VBS or Hyper-V) already uses it.

## Options

| | Option | What it proves | What must be enabled (a user decision) | Cost and risk |
|---|---|---|---|---|
| A | **Windows Sandbox** | A clean Windows per run: MSI install, upgrade, downgrade, uninstall, Startup-folder and Run-key writes, first logon of a fresh user. Discarded at close. | Feature `Containers-DisposableClientVM`, one reboot. Needs Pro (ok). | Cheap and disposable. Sandbox shares no state with the host unless a folder is mapped (map read-only). Cannot test a reboot (closing it discards everything), so autostart at a real logon is not proven. |
| B | **Hyper-V VM** | Everything in A, plus reboots and real logon autostart, snapshots to roll back, upgrade across versions. | Feature `Microsoft-Hyper-V` (reboot), a Windows 11 evaluation or licensed ISO, ~40 GB disk. | Heaviest. Needs a licence/ISO decision. Best for the "does it start after a reboot" claim. |
| C | **Throwaway local user** on the real PC | Per-user things only: Startup folder, HKCU Run, first run. | Creating an account (system change). | Poor fit: a per-machine MSI install, upgrade or uninstall changes `Program Files`, HKLM and services for everyone, so it is the user's real PC. Avoid. |
| D | **Offline MSI table inspection** | What the package says without installing: the Upgrade/Component/Registry/Shortcut tables, `MajorUpgrade` and downgrade rules, autostart entries, UpgradeCode and version ordering. | Nothing. Read the .msi with the Windows Installer COM API (`WindowsInstaller.Installer`, read-only open) or `msiinfo`/`msitools`-style parsing. | Window-free and safe. Proves the package's declared behaviour, not that Windows executes it correctly. |
| E | **WinPE / offline image** | A mounted image can be inspected; nothing installs there. | `dism`/ADK. | Not worth it: it adds nothing over D. |

## Recommendation

1. **Now, no decision needed: D.** Write a window-free test that opens the built MSI read-only and
   asserts: UpgradeCode and ProductVersion ordering, the upgrade and downgrade rows
   (`MajorUpgrade` with downgrade blocked or allowed as intended), the Startup shortcut or Run value
   it creates and removes, and that uninstall removes what install added. This runs in the normal
   gate and never installs anything.
2. **Best dynamic option: A (Windows Sandbox).** It covers install, upgrade, downgrade, uninstall and
   autostart file/registry moves on a clean machine and is thrown away at close. It needs the
   `Containers-DisposableClientVM` feature and a reboot, which the user decides.
3. **Only if real reboot or logon autostart proof is required: B (Hyper-V).** A VM with a snapshot
   can reboot and log on. It needs the feature, an ISO/licence and disk.
4. **Avoid C** for anything per-machine.

## Decisions for the user

- Enable Windows Sandbox (A)? One reboot; agents cannot do this (system setting).
- Is real logon autostart proof needed, which means Hyper-V and a Windows ISO (B)?

## What agents must not do

- Enable any Windows feature, create accounts, or change system settings to run these tests.
- Install or uninstall the MSI on the real PC to "just try it".
- Map writable host folders into a Sandbox or VM, or put the user's files or tokens in one.
- Start a Sandbox or VM window on the user's desktop while they may be gaming (game-check first);
  a Sandbox window is a normal window, so it needs the same care as any launched app.
