## Interactive verification environment

Run:

```sh
make test-inter
```

The target builds `dtm`, creates a test image containing the release binary and
files from `test/fixtures/home`, then starts a shell as an unprivileged `dtm`
user. The container is started with `--rm`, so changes made inside the shell are
discarded when you exit.

Inside the shell, the test fixture is the user's home directory:

```sh
printf '%s\n' "$HOME"
find "$HOME" -maxdepth 3 -type f -print
which dtm
```

The target uses Podman by default. The image is rebuilt on the next invocation
when the binary or fixture files change. Podman may reuse the base image and
package layers, but no container state is reused.

To use another container engine or a different local image name:

```sh
make test-inter CONTAINER_ENGINE=docker IMAGE=dtm-test-inter:debug
```

Run and apply one package:

```sh
dtm stow <PACKAGE>
dtm stow --pkgs-dir /path/to/dotfiles <PACKAGE>
```

The `stow` command prints one tab-separated line per scanned file containing its
source path, target path, and type (`symlink` or `template`), then applies the
plan. Use `--dry-run` to only print the plan. These options belong only to the
`stow` command.

Existing target files are handled as follows:

```text
(default)              warn and skip a different target
-s, --semi-force       replace a different symbolic link; keep a regular file
-f, --force            replace a different symbolic link or regular file
```

Add `-b` or `--backup` to move targets that `--semi-force` or `--force` would
otherwise delete. Normal mode still skips conflicts, so backup has no effect in
normal mode. Backups require `config.backup_dir` and use a fixed reverse mapping:

```text
<backup_dir>/<PACKAGE>/<NEAREST_VARIABLE>/<reverse-mapped-relative-path>
```

For each target, dtm selects the configured variable whose absolute path is the
nearest parent of that target. For example, with `home=/home/user` and
`config_home=/home/user/.config`, a conflict at
`/home/user/.config/app/settings` is backed up below
`<backup_dir>/<PACKAGE>/config_home/app/settings`, not below
`home/dot-config/app/settings`.

Every hidden relative path component is converted from a leading `.` to
`dot-`, so `${home}/.gitconfig` becomes
`<backup_dir>/git/home/dot-gitconfig` and
`${config_home}/app/.state` becomes
`<backup_dir>/<PACKAGE>/config_home/app/dot-state`. The mapping is fixed and
reversible. If the mapped backup path already exists, stow fails instead of
overwriting it or creating a numbered backup; restore the existing backup
first.

Restore a package backup with:

```sh
dtm restore <PACKAGE>
```

Restore first scans the complete package backup and validates every entry before
changing any target. Each backup must map to an entry in the package's current
plan. A managed symlink target must still point to that entry's package source;
a managed template target must still be a regular file whose bytes match the
currently rendered template. If any target is missing, modified, points
elsewhere, or any backup entry cannot be mapped, the whole restore is rejected
without moving any backup. After a successful preflight, dtm removes the managed
targets, moves the backups into their original locations, and removes the now
empty package backup directories.

A template is rendered before the target is checked. A regular target whose
bytes already match the rendered template is left unchanged.


By default, `dtm` loads:

```text
$XDG_CONFIG_HOME/dtm/config.yaml
```

When `XDG_CONFIG_HOME` is not set, it uses `~/.config/dtm/config.yaml`.
Use the shared `--config` option before `stow`, `restore`, or `config` to
select a different file:

```sh
dtm --config /path/to/config.yaml stow <PACKAGE>
dtm --config /path/to/config.yaml restore <PACKAGE>
dtm --config /path/to/config.yaml config get pkgs_dir
```

Persist or inspect runtime directories with:

```sh
dtm config set pkgs_dir /path/to/dotfiles
dtm config get pkgs_dir
dtm config set backup_dir /path/to/backups
dtm config get backup_dir
dtm config list
```

`config list` prints only keys explicitly present in the configuration file,
using tab-separated columns. It lists `pkgs_dir` and `backup_dir` when they are
configured.

`config set pkgs_dir` resolves its input to an existing absolute directory.
`config set backup_dir` resolves its input to an absolute path and creates the
directory when needed. Package deployment options such as `--pkgs-dir`,
`--dry-run`, backup, and force modes are accepted only by `stow`; `restore`
uses the configured package and backup directories.

Configuration uses YAML with separate runtime configuration and template
variables:

```yaml
config:
  pkgs_dir: /home/user/dotfiles
  backup_dir: /home/user/.local/state/dtm/backups

variables:
  config_home: ${home}/.config
  local_bin: ${home}/.local/bin
  system_config: ${root}/etc/dtm
```

`config.pkgs_dir` must be an absolute path. It defaults to the current working
directory when omitted. The `--pkgs-dir` option overrides it for the current run
without changing the configuration file:

```sh
dtm stow --pkgs-dir /path/to/dotfiles <PACKAGE>
```

`home` defaults to the current user's home directory and `root` defaults to the
filesystem root (`/`). Both are normal variables and can be overridden. A value
may reference another declared variable, such as `${config_home}/dtm`. Unknown
variables, malformed references, and reference cycles are rejected.
