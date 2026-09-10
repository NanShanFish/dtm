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
<backup_dir>/<PACKAGE>/<NEAREST_PATH>/<reverse-mapped-relative-path>
```

For each target, dtm selects the configured path whose absolute value is the
nearest parent of that target. Ordinary `variables` are never considered during
this filesystem lookup. For example, with `home=/home/user` and
`config_home=/home/user/.config` in the `path` block, a conflict at
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

Remove an installed package with:

```sh
dtm rm <PACKAGE>
```

Removal is safe by default. Before deleting anything, dtm checks every entry in
the current package plan with the same ownership test used for idempotent stow:
a symlink must still point to its package source, and a rendered template must
still have exactly the expected contents. A missing, modified, replaced, or
redirected target is not managed by dtm. If any such target exists, the entire
remove operation stops without deleting any package entry.

Use `--skip-unmanaged` to leave those targets untouched and remove only entries
that still pass the dtm ownership check:

```sh
dtm rm --skip-unmanaged <PACKAGE>
```

Removal deletes only managed files and symbolic links. It does not recursively
delete destination directories.

Restore a package backup with:

```sh
dtm restore <PACKAGE>
```

Restore first scans the complete package backup and validates that every backup
maps to the package's current plan. It then performs the same safe, whole-package
removal as `dtm rm`; this preflight checks every planned target, including
installed entries that have no backup. If any target is missing, modified,
points elsewhere, or any backup entry cannot be mapped, the whole restore is
rejected without deleting a target or moving a backup.

After both preflights succeed, dtm uninstalls the complete package, moves the
backed-up files into their original locations, and removes the now-empty package
backup directories. Consequently, package entries without backups remain
uninstalled after restore; only the original conflicting files are restored.

A template is rendered before the target is checked. A regular target whose
bytes already match the rendered template is left unchanged.


By default, `dtm` loads:

```text
$XDG_CONFIG_HOME/dtm/config.yaml
```

When `XDG_CONFIG_HOME` is not set, it uses `~/.config/dtm/config.yaml`.
Use the shared `--config` option before `stow`, `rm`, `restore`, or `config` to
select a different file:

```sh
dtm --config /path/to/config.yaml stow <PACKAGE>
dtm --config /path/to/config.yaml rm <PACKAGE>
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
using tab-separated columns. Its values are resolved through `path` and
`variables`, so `${home}/dotfiles` is printed as the actual absolute path. It
lists `pkgs_dir` and `backup_dir` when they are configured.

`config set pkgs_dir` resolves its input to an existing absolute directory.
`config set backup_dir` resolves its input to an absolute path and creates the
directory when needed. Deployment options such as `--pkgs-dir`, `--dry-run`,
backup, and force modes are accepted only by `stow`. The `--skip-unmanaged`
option is accepted only by `rm`; `rm` and `restore` use the configured package
directory, and `restore` also uses the configured backup directory.

`config.pkgs_dir` must be an absolute path. It defaults to the current working
directory when omitted. The stow-only `--pkgs-dir` option overrides it for the
current run without changing the configuration file:

```sh
dtm stow --pkgs-dir /path/to/dotfiles <PACKAGE>
```

Configuration uses YAML with separate runtime configuration, filesystem paths,
and ordinary template variables:

```yaml
config:
  pkgs_dir: ${home}/dotfiles
  backup_dir: ${home}/.local/state/dtm/backups

path:
  config_home: ${home}/.config
  local_bin: ${home}/.local/bin
  system_config: ${root}/etc/dtm
  themed_config: ${config_home}/${theme}

variables:
  theme: dark
  profile: personal
```

`path` contains filesystem destination roots. Every resolved path must be
absolute. Package first-level directories must name an entry in `path`, and
backup nearest-parent matching plus restore lookup inspect only this block.
`home` defaults to the current user's home directory and `root` defaults to the
filesystem root (`/`); both are predefined paths and may be overridden in
`path`.

`variables` contains ordinary string values and is not inspected by filesystem
operations, even when a string happens to look like an absolute path. The two
blocks share interpolation resolution, so a path can reference a variable and
a variable can reference a path. Defining the same name in both blocks is an
error because template references would be ambiguous.

Templates receive the merged values from both blocks. Thus both
`{=config_home=}` and `{=theme=}` are valid. Unknown references, malformed
references, relative values in `path`, and cycles across either block are
rejected.
