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

## Configuration

By default, `dtm` loads:

```text
$XDG_CONFIG_HOME/dtm/config.yaml
```

When `XDG_CONFIG_HOME` is not set, it uses `~/.config/dtm/config.yaml`.
A different file can be selected with:

```sh
dtm --config /path/to/config.yaml
dtm --config=/path/to/config.yaml
```

Configuration uses YAML. Variable values are plain scalars and support limited,
non-shell interpolation:

```yaml
variables:
  config_home: ${home}/.config
  local_bin: ${home}/.local/bin
  system_config: ${root}/etc/dtm
```

`home` defaults to the current user's home directory. `root` defaults to the
filesystem root (`/`). Both are normal variables and can be overridden in the
configuration. A value may reference another declared variable, such as
`${config_home}/dtm`. Unknown variables, malformed references, and reference
cycles are rejected.
