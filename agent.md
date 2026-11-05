# Build caches

- Never place build caches in memory or on RAM-backed filesystems (such as tmpfs, `/dev/shm`, or RAM-backed `/tmp` directories). Always use disk-backed storage for build caches and compiler temporary files.
