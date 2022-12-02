# Duplicate file finder

This utility recursively searching directories for duplicate files (exact
content matches). Using the `--symlink` option, duplicate files are replaced by
a relative symlink to a matching file. Alternatively, specifying the `--remove`
option removes duplicates.

Note: this utility is relatively untested and should be considered
experimental.

### Usage

```
Find duplicate files in a directory structure

Usage: dedup [OPTIONS] <PATHS>...

Arguments:
  <PATHS>...  Directories to search

Options:
  -m, --min-size <MIN_SIZE>    Minimum size (in bytes) of files to search [default: 0]
  -v, --verbose                Print file names and sizes of the found duplicates
  -d, --max-depth <MAX_DEPTH>  Do not search files beyond this depth. Files in the specified paths are considered depth 1.
  -s, --symlink                Replace duplicate files by symlinks
      --remove                 Remove duplicate files
  -h, --help                   Print help information
```

### Algorithm

The tool tries to be relatively efficient, by first making an index of file
sizes mapping to paths. If a second file is found with the same file size, the
first 64 KiB of the files are hashed using SHA-256, and stored into a second
index of files with that size. Only once a hash collision is found for two
files that have identical starts, are the full contents of the files hashed and
compared.


### License

Licensed under the [Apache 2 License](LICENSE).
