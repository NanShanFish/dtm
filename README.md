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
dtm --dot-dir /path/to/dotfiles <PACKAGE>
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
A different file can be selected with:

```sh
dtm --config /path/to/config.yaml
```

Configuration uses YAML. Variable values are plain scalars and support limited,
non-shell interpolation:

```yaml
variables:
  config_home: ${home}/.config
  local_bin: ${home}/.local/bin
  system_config: ${root}/etc/dtm
  _dotfile_dir: ${home}/dotfiles
```

`home` defaults to the current user's home directory. `root` defaults to the
filesystem root (`/`). `_dotfile_dir` defaults to the current working directory.
All three are normal variables and can be overridden in the configuration. The
`--dot-dir` command-line option has the highest priority and overrides the
`_dotfile_dir` value from the configuration:

```sh
dtm --dot-dir /path/to/dotfiles
```

A value may reference another declared variable, such as
`${_dotfile_dir}/packages`. Unknown variables, malformed references, and
reference cycles are rejected.
