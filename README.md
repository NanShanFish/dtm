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
dtm <PACKAGE>
dtm --pkgs-dir /path/to/dotfiles <PACKAGE>
```

The command prints one tab-separated line per scanned file containing its source
path, target path, and type (`symlink` or `template`), then applies the plan.
Use `--dry-run` to only print the plan.

Existing target files are handled as follows:

```text
(default)       warn and skip a different target
--semi-force    replace a different symbolic link; keep a regular file
--force         replace a different symbolic link or regular file
```

A template is rendered before the target is checked. A regular target whose
bytes already match the rendered template is left unchanged.


By default, `dtm` loads:

```text
$XDG_CONFIG_HOME/dtm/config.yaml
```

When `XDG_CONFIG_HOME` is not set, it uses `~/.config/dtm/config.yaml`.
Use the shared `--config` option before either a package or config command to
select a different file:

```sh
dtm --config /path/to/config.yaml <PACKAGE>
dtm --config /path/to/config.yaml config get pkgs_dir
```

Persist or inspect the packages directory with:

```sh
dtm config set pkgs_dir /path/to/dotfiles
dtm config get pkgs_dir
dtm config list
```

`config list` prints only keys explicitly present in the configuration file,
using tab-separated columns. It currently lists `pkgs_dir` when configured.

`config set pkgs_dir` accepts a relative input path, resolves it to an existing
absolute directory, and writes that path to `config.pkgs_dir`. Package-only
options such as `--pkgs-dir`, `--dry-run`, and force modes are not accepted by
config commands.

Configuration uses YAML with separate runtime configuration and template
variables:

```yaml
config:
  pkgs_dir: /home/user/dotfiles

variables:
  config_home: ${home}/.config
  local_bin: ${home}/.local/bin
  system_config: ${root}/etc/dtm
```

`config.pkgs_dir` must be an absolute path. It defaults to the current working
directory when omitted. The `--pkgs-dir` option overrides it for the current run
without changing the configuration file:

```sh
dtm --pkgs-dir /path/to/dotfiles <PACKAGE>
```

`home` defaults to the current user's home directory and `root` defaults to the
filesystem root (`/`). Both are normal variables and can be overridden. A value
may reference another declared variable, such as `${config_home}/dtm`. Unknown
variables, malformed references, and reference cycles are rejected.
