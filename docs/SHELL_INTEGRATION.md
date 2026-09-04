# Scry Console Shell Integration

Scry can browse and search files directly on a FreeBSD system console. Shell
integration adds the ability to leave Scry in the directory selected during
that session.

## Why shell integration is required

Scry runs as a child of your interactive shell. A child process cannot change
the working directory of its parent process, so the `scry` executable cannot
move the shell by itself.

The installed helper solves this safely:

1. The helper creates a private temporary handoff file.
2. It launches the real `scry` executable with that filename in
   `SCRY_CD_FILE`.
3. Scry writes a requested local destination only when you choose one.
4. After Scry exits, the helper validates the destination and changes the
   current shell directory.
5. The temporary file is removed.

The helper does not spawn a nested shell and Scry does not edit any shell
startup file.

## Console actions

With shell integration enabled on the FreeBSD system console:

| Action | Result |
| --- | --- |
| Enter on a local file | Exit Scry in the file's containing directory |
| Enter on a local directory | Browse into the directory inside Scry |
| F3 on a local directory | Exit Scry in the selected directory |
| F3 on a local file | No action |
| F3 while browsing through SFTP | No action |

Rich-terminal behavior is unchanged. In rich terminals, Enter continues to
use Scry's ordinary activation and file-opening behavior, and F3 continues to
control icons.

## Installed helper files

The FreeBSD package installs:

```text
/usr/local/share/scry/shell/scry.sh
/usr/local/share/scry/shell/scry.csh
```

Use `scry.sh` with `sh`, `bash`, `ksh`, and `zsh`. Use `scry.csh` with `csh`
and `tcsh`.

Choose your shell below and add only the shown integration line to its startup
file. Existing startup content should remain in place.

## sh

Add this line to `~/.profile`:

```sh
. /usr/local/share/scry/shell/scry.sh
```

Load it into the current shell:

```sh
. ~/.profile
```

## bash

Add this line to `~/.bashrc`:

```bash
. /usr/local/share/scry/shell/scry.sh
```

Load it into the current shell:

```bash
. ~/.bashrc
```

If login Bash sessions do not already read `~/.bashrc`, ensure that
`~/.bash_profile` contains:

```bash
if [ -f "$HOME/.bashrc" ]; then
    . "$HOME/.bashrc"
fi
```

## ksh

Korn shell reads the startup file named by `ENV`. A common arrangement is to
add this to `~/.profile`:

```ksh
export ENV="$HOME/.kshrc"
```

Then add this line to `~/.kshrc`:

```ksh
. /usr/local/share/scry/shell/scry.sh
```

Start a new Korn shell after setting `ENV`, or load the helper directly for the
current shell:

```ksh
. /usr/local/share/scry/shell/scry.sh
```

If your Korn shell already uses a different `ENV` file, add the helper line to
that file instead of replacing the existing setting.

## zsh

Add this line to `~/.zshrc`:

```zsh
. /usr/local/share/scry/shell/scry.sh
```

Load it into the current shell:

```zsh
. ~/.zshrc
```

## csh

Add this alias to `~/.cshrc`:

```csh
alias scry 'source /usr/local/share/scry/shell/scry.csh \!*'
```

Load it into the current shell:

```csh
source ~/.cshrc
```

## tcsh

Add this alias to `~/.tcshrc`:

```tcsh
alias scry 'source /usr/local/share/scry/shell/scry.csh \!*'
```

Load it into the current shell:

```tcsh
source ~/.tcshrc
```

If `~/.tcshrc` does not exist and your setup uses `~/.cshrc`, place the alias
in `~/.cshrc` instead.

## Verify the integration

For `sh`, `bash`, or `ksh`:

```sh
type scry
```

For `zsh`:

```zsh
whence -v scry
```

These should report that `scry` is a shell function.

For `csh` or `tcsh`:

```csh
alias scry
```

This should print the Scry alias and its `source` command.

Now launch Scry through the helper:

```text
scry
```

On the FreeBSD system console, select a local file and press Enter. Scry should
exit and `pwd` should report the file's containing directory. Launch Scry again,
select a local directory, and press F3. Scry should exit in that directory.

Launching `/usr/local/bin/scry` directly bypasses the shell helper. Scry can
still browse in that form, but it cannot change the parent shell directory.

## Command-line arguments

The helpers pass every command-line argument to the real executable. Existing
commands continue to work normally, including:

```text
scry /usr/local
scry -r --query 'ext:rs +session' ~/Projects
scry --ssh example-host
scry --manual
scry --console-config
```

## Development builds

Developers can point either helper at a locally built executable with
`SCRY_BINARY`.

For `sh`, `bash`, `ksh`, or `zsh`, set the variable before sourcing `scry.sh`:

```sh
SCRY_BINARY="$HOME/bsd_tools/scry/target/release/scry"
. "$HOME/bsd_tools/scry/shell/scry.sh"
```

For `csh` or `tcsh`:

```csh
setenv SCRY_BINARY "$HOME/bsd_tools/scry/target/release/scry"
alias scry 'source "$HOME/bsd_tools/scry/shell/scry.csh" \!*'
```

`SCRY_BINARY` is not needed for the installed FreeBSD package. Remove that
development setting after switching to the packaged executable.

## Remote browsing

Shell handoff is intentionally disabled while Scry is browsing through its
built-in SSH/SFTP source. A local shell cannot change directory into a path on
another computer.

If you first connect to the remote computer with `ssh` and run an installed and
configured Scry there, the feature works normally because Scry and the shell
then share that computer's local filesystem.

## Troubleshooting

### Scry says to launch through its shell helper

The executable was launched directly or the helper was not loaded. Reload the
appropriate startup file and run `scry`, not `/usr/local/bin/scry`.

### Enter on a file does not leave Scry

Confirm that you are on the FreeBSD system console and that `scry` resolves to
the function or alias described under **Verify the integration**. Rich-terminal
file activation remains unchanged intentionally.

### F3 does nothing

F3 requests a shell destination only when a local directory is selected in
console mode. It deliberately does nothing on files and remote SFTP entries.

### The shell reports `^M`, `Command not found`, or syntax errors

The startup file or helper may contain Windows CRLF line endings. Convert it to
Unix LF line endings and reload the shell configuration.

### A development binary produces `Exec format error`

Rebuild it on FreeBSD with:

```text
cargo build --release
```

Then confirm that `target/release/scry` is a native FreeBSD executable before
using it through `SCRY_BINARY`.

## Remove the integration

Delete the Scry source line or alias from the shell startup file and start a new
shell. No other user files are created by the helpers.

The installed helper files belong to the Scry package and are removed when the
package is uninstalled.
