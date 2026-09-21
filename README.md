# dtm

[简体中文](docs/README.zh_CN.md)

dtm, d(o)t(file)m(anager), is a tool written in Rust for managing dotfiles on 
Unix systems.

## Installation

```bash
git clone https://github.com/nanshanfish/dtm.git
cd dtm
sudo -E make install
```

## Command Overview

| Command | Description |
| --- | --- |
| [`dtm stow [OPTIONS] <PACKAGE>...`](#stowing-packages) | Install one or more packages. |
| [`dtm pack <PACKAGE> <PATH>...`](#packing) | Move existing files into a package and install it. |
| [`dtm pack -i <PACKAGE> <DIRECTORY>`](#packing) | Interactively select files from a directory to pack. |
| [`dtm rm [--skip-unmanaged] <PACKAGE>...`](#removing-packages) | Remove one or more installed packages. |
| [`dtm restore <PACKAGE>`](#restoring-backups) | Remove a package and restore its backed-up files. |
| [`dtm config set/get/list`](#configuration) | Set, inspect, or list configuration values. |

## Usage

Given a directory with the following structure:

```
<pkgs_dir>/
    fish/
        config_home/
            config.tmpl.fish
            conf.d/
                interactive.fish
        home/
            other_file
```

When you run `dtm stow fish`, dtm looks for a subdirectory named `fish` under 
the configured `pkgs_dir` directory. For each subdirectory of `fish`, dtm looks 
for a corresponding entry in the configuration's `path` section. For example, 
with the following configuration:

```
path:
    config_home: ${home}/.config
```

The predefined `home` value uses the user's home directory by default, so 
`config_home` resolves to `~/.config`. dtm therefore creates a symbolic link at 
`~/.config/fish/conf.d/interactive.fish` that points to 
`<pkgs_dir>/fish/config_home/conf.d/interactive.fish`.

Files whose names contain `.tmpl` are handled differently. dtm searches their 
contents for fields enclosed by `{==}` and fills them using values from the 
configuration's `variable` section. It then writes the rendered file to the 
corresponding path after removing `.tmpl` from the filename. For example, 
`<pkgs_dir>/fish/config_home/config.tmpl.fish` is rendered to 
`~/.config/fish/config.fish`.

The default configuration path is `$XDG_CONFIG_HOME/dtm/config.yaml` when 
`XDG_CONFIG_HOME` is set, or `$HOME/.config/dtm/config.yaml` otherwise. You can 
override it by placing `--config <PATH>` before any subcommand.

## Stowing Packages

```bash
dtm stow [--pkgs-dir <PATH>] [-s | --semi-force] [-f | --force] [-b | --backup] [--dry-run] <pkg_name1> <pkg_name2> ...
```

Options:

- `--pkgs-dir <PATH>`: directly specify the location of `pkgs_dir`.
- `-s / --semi-force`: remove a conflicting target when it is a symbolic link 
not managed by dtm.
- `-f / --force`: remove a conflicting target not managed by dtm.
- `-b / --backup`: move conflicting files to `backup_dir` before removal; 
requires the `backup_dir` configuration field.
- `--dry-run`: show the planned operations without making changes.

### Packing

```bash
dtm pack <pkg_name> <PATH>...
dtm pack [-i | --interactive] <pkg_name> <DIRECTORY>
```

This command moves files from the specified paths into the corresponding 
locations in dtm's package structure, then creates links at their original 
locations.

When packing, dtm uses the most specific configured path whenever possible. For 
example, when running `dtm pack fish ~/.config/fish/config.fish`, if the 
configuration defines `config_home: ${home}/.config`, the file is moved to 
`<pkgs_dir>/fish/config_home/fish/config.fish` rather than 
`<pkgs_dir>/fish/home/dot-config/fish/config.fish`.

Similarly, if `fish_config: ${home}/.config/fish` is also defined, the file is 
moved to `<pkgs_dir>/fish/fish_config/config.fish`.

Options:

- `-i / --interactive`: accepts exactly one `DIRECTORY` argument and opens an 
interactive interface for selecting which files to pack. This is useful when 
you want to selectively pack only some configuration files from a directory. 
The command ignores common dependency directories and VCS directories such as 
`.git` while scanning. Files and directories passed directly as arguments are 
not ignored.

### Removing Packages

```bash
dtm rm [--skip-unmanaged] <pkg_name1> <pkg_name2>
```

Before removal, dtm verifies that every destination corresponding to a 
dtm-managed file is still managed by dtm. If any destination is no longer 
managed by dtm, the command exits without removing the package. If every 
destination is still managed, the corresponding files are deleted. Use 
`--skip-unmanaged` to bypass this strict validation and delete only files that 
are still managed by dtm.

### Restoring Backups

```bash
dtm restore <pkg_name>
```

This command removes the package and restores the original files backed up by 
the `-b` option of the `stow` command.

## Configuration

The following is an example configuration. The predefined `root` and `home` 
paths default to the filesystem root and the user's home directory, 
respectively, and can be overridden in the configuration file. Environment 
variables can be referenced using syntax such as `${$HOME}`.

```yaml
path:
  config_home: ${home}/.config
  local_bin: ${home}/.local/bin
  system_config: ${root}/etc/dtm

variables:
  example_name: dtm

config:
  pkgs_dir: ${home}/dot/pkgs
  backup_dir: ${home}/.local/backup
```

## TODO

- [ ] Add package hooks such as `pre-install`.
- [ ] Support Windows.
- [ ] Add package dependencies.

## Contributing

Pull requests are welcome. For major changes, please open an issue first
to discuss what you would like to change.

Please make sure to update tests as appropriate.

### Interactive Testing

```bash
make test-inter
```

Use this command to start an interactive test environment. It creates a 
disposable container image and removes the container when the session ends. 
Test commands run inside the container.

## Acknowledgments

GNU Stow, for inspiring dtm's package-management design.
