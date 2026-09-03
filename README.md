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
