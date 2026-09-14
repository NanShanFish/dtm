# dtm

[English](../README.md)

dtm, d(o)t(file)m(anager) 是一个用 Rust 编写的用来在 Unix 上管理 dotfile 的工具.

## 安装

```bash
git clone https://github.com/nanshanfish/dtm.git
cd dtm
cargo build --release
cp target/release/dtm /usr/local/bin/
```

## 命令总览

| 命令 | 说明 |
| --- | --- |
| [`dtm stow [OPTIONS] <PACKAGE>...`](#链接包) | 安装一个或多个包。 |
| [`dtm pack <PACKAGE> <PATH>...`](#打包) | 将已有文件移动到包中并安装该包。 |
| [`dtm pack -i <PACKAGE> <DIRECTORY>`](#打包) | 通过交互界面选择目录中的文件并打包。 |
| [`dtm rm [--skip-unmanaged] <PACKAGE>...`](#卸载包) | 卸载一个或多个包。 |
| [`dtm restore <PACKAGE>`](#恢复备份) | 卸载包并恢复备份文件。 |
| [`dtm config set/get/list`](#配置) | 设置、查询或列出配置值。 |

## 用法

对于目录结构为以下的目录
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
当运行 `dtm stow fish` 时, dtm 会在配置里的 `pkgs_dir` 文件夹下查找名为 fish 
的子文件夹, 对于 fish 的子文件夹, dtm 会尝试在配置里的 `path` 字段中匹配它们. 
例如如果配置了
```
path:
    config_home: ${home}/.config
```
默认 `home` 变量会使用 `HOME` 环境变量的值, 所以此时 `config_home` 的值为 
`~/.config`, 因此会在 `~/.config/fish/conf.d/interactive.fish` 创建指向 
`<pkgs_dir>/fish/config_home/conf.d/interactive.fish` 的软链接.
对于名字中有 `.tmpl` 的文件要特殊一些, dtm 会在配置里的 `variable` 搜索文件
中以 `{==}` 包裹的 字段, 并在填充值后将填充后的文件写入到去除 `.tmpl` 后的
对应位置, 例如 `<pkgs_dir>/fish/config_home/config.tmpl.fish` 会填充后写入到
`~/.config/fish/config.fish`


配置文件的默认位置在设置了 `XDG_CONFIG_HOME` 时为 
`$XDG_CONFIG_HOME/dtm/config.yaml`, 否则为 `$HOME/.config/dtm/config.yaml`; 
可在所有子命令前使用 `--config <PATH>` 覆盖

## 链接包

```bash
dtm stow [--pkgs-dir <PATH>] [-s | --semi-force] [-f | --force] [-b | --backup] [--dry-run] <pkg_name1> <pkg_name2> ...
```
选项:
- `--pkgs-dir <PATH>` 直接指定 `pkgs_dir` 位置
- `-s / --semi-force` 如果目标处对应文件非dtm在管理且为软链接则删除
- `-f / --force`      删除目标处对应文件非dtm在管理则删除
- `-b / --backup`     移动冲突文件到 `backup_dir` 后再删除, 需要配置 
`backup_dir` 字段
- `--dry-run`         查看预期行为, 不实际运行

### 打包

```bash
dtm pack <pkg_name> <PATH>...
dtm pack [-i | --interactive] <pkg_name> <DIRECTORY>
```
会把对应路径的文件以 dtm 的组织方式移动到对应的位置, 然后在原位置处创建链接 dtm 
打包时会尽量使用最小的文件层级 例如当运行 `dtm pack fish 
~/.config/fish/config.fish` 时 如果配置里定义了 `config_home: ${home}/.config`, 
则会将文件移动到 `<pkgs_dir>/fish/config_home/fish/config.fish` , 而不是 
`<pkgs_dir>/fish/home/dot-config/fish/config.fish`, 同理如果还定义了 
`fish_config: ${home}/.config/fish` 则会将其移动到 
`<pkgs_dir>/fish/fish_config/config.fish`

选项:
- `-i / --interactive` 使用此选项时只接受一个 `DIRECTORY` 参数, 
会启动一个交互式界面选择打包哪些文件 
对于要选择性地打包一个文件夹下的部分配置文件时很有用, 
该命令会忽略文件夹中常见的依赖文件或者VCS文件(.git) 
若这些文件/文件夹直接作为参数传递则不忽略

### 卸载包

```bash
dtm rm [--skip-unmanaged] <pkg_name1> <pkg_name2>
```
运行前会先验证 dtm 管理的文件的对应位置是否为 dtm 在管理, 若存在非 dtm 
管理的文件则 退出, 均为 dtm 在管理则删除对应文件; 使用 `--skip-unmanaged` 
选项可超过验证, 只 删除 dtm 在管理的文件

### 恢复备份

```
dtm restore <pkg_name>
```
卸载包并恢复使用 `stow` 命令 `-b` 选项创建的原文件的备份

## 配置

配置文件样例, 其中 `root` 和 `home` 变量分别使用根目录和家目录作为默认值 
可在配置文件里再次定义覆盖, 配置文件中可以使用 `${$HOME}` 形式引用环境变量
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

## 待办

- [ ] 为包添加 `pre-install` 等脚本 hook 功能
- [ ] 支持 Windows
- [ ] 添加包依赖关系

## 贡献

欢迎为该仓库提交 PR, 对于大的变更, 请先创建一个 issue 讨论修改的内容

提交前请确保更新相关测试用例

### 交互式测试

```bash
make test-inter
```
可以使用该命令启动一个交互式测试环境, 该命令会创建一个容器镜像并在结束会删除, 
测试命令在容器里面运行


## 鸣谢

GNU Stow, 感谢 stow 在包管理方面给予的启发
