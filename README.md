# Pixiv Archive

> Check [PostArchiver](https://github.com/xiao-e-yun/PostArchiver) to know more info.

It is importer for pixiv to PostArchiver.

```sh
Usage: pixiv-archive [OPTIONS] <SESSION> [OUTPUT]

Arguments:
  <SESSION>  Your `PHPSESSID` cookie [env: PHPSESSID=]
  [OUTPUT]   Which you path want to save [env: OUTPUT=] [default: ./archive]

Options:
      --users [<USERS>...]                  archive Id of Users
      --illusts [<ILLUSTS>...]              archive Id of Illusts
      --novels [<NOVELS>...]                archive Id of Novels
      --illust-series [<ILLUST_SERIES>...]  archive Id of Illust Series
      --novel-series [<NOVEL_SERIES>...]    archive Id of Novel Series
      --followed-users                      archive followed users
      --favorite                            archive favorite artworks
  -o, --overwrite                           Overwrite existing files
  -u, --user-agent <USER_AGENT>             [default: ]
  -l, --limit <LIMIT>                       Limit the number of concurrent copys [default: 40]
  -v, --verbose...                          Increase logging verbosity
  -q, --quiet...                            Decrease logging verbosity
  -h, --help                                Print help
```

## Build

How to build & run code
```sh
cargo run
```
