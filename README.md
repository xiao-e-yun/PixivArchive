# Pixiv Archive

> Check [PostArchiver](https://github.com/xiao-e-yun/PostArchiver) to know more info.

It is importer for pixiv to PostArchiver.

```sh
Usage: pixiv-archive [OPTIONS] <SESSION>

Arguments:
  <SESSION>
          Your `PHPSESSID` cookie
          
          [env: PHPSESSID]

Options:
      --users [<USERS>...]
          archive Id of Users

      --illusts [<ILLUSTS>...]
          archive Id of Illusts

      --novels [<NOVELS>...]
          archive Id of Novels

      --illust-series [<ILLUST_SERIES>...]
          archive Id of Illust Series

      --novel-series [<NOVEL_SERIES>...]
          archive Id of Novel Series

      --followed-users
          archive followed users

      --favorite
          archive favorite artworks

  -o, --output <OUTPUT>
          Archive directory to save into
          
          [env: OUTPUT=]
          [default: ./archive]

      --strategy <STRATEGY>
          Archiving strategy (`full` re-archives existing posts)
          
          [default: increment]

          Possible values:
          - increment: Only fetch posts newer than the last run, and skip posts already in the archive
          - full:      Fetch everything again and re-import posts already in the archive

  -u, --user-agent <USER_AGENT>
          [default: ]

  -l, --limit <LIMIT>
          Limit API requests per minute
          
          [default: 60]

  -v, --verbose...
          Increase logging verbosity

  -q, --quiet...
          Decrease logging verbosity

  -h, --help
          Print help (see a summary with '-h')
```

## Build

How to build & run code
```sh
cargo run
```
