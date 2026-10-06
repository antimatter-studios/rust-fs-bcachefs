/*
 * fs_bcachefs.h -- read-only C ABI of rust-fs-bcachefs (MIT).
 *
 * Every function catches failures: one returning int or int64_t returns -1,
 * one returning a pointer returns NULL, and fs_core_last_error_message()
 * (fs_core.h) says why. Nothing here writes to the filesystem.
 */
#ifndef FS_BCACHEFS_H
#define FS_BCACHEFS_H

#include <stddef.h>
#include <stdint.h>
#include "fs_core.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct fs_bcachefs_fs fs_bcachefs_fs;

typedef struct {
    uint64_t ino;
    uint32_t mode;    /* st_mode: type and permission bits */
    uint32_t nlink;
    uint32_t uid;
    uint32_t gid;
    uint64_t size;    /* bytes */
    uint64_t sectors; /* 512-byte sectors allocated */
} fs_bcachefs_attr_t;

/* Return non-zero to stop the listing. d_type uses the DT_* numbering. */
typedef int (*fs_bcachefs_dirent_cb)(void *ctx, const uint8_t *name, size_t name_len,
                                     uint64_t ino, uint8_t d_type);

/* Open an image or device read-only. NULL on failure. */
fs_bcachefs_fs *fs_bcachefs_mount(const char *path);
/* Close a handle; NULL is a no-op. */
void fs_bcachefs_umount(fs_bcachefs_fs *fs);
/* Stat an absolute path, not following a final symlink. 0 or -1. */
int fs_bcachefs_stat(fs_bcachefs_fs *fs, const char *path, fs_bcachefs_attr_t *out);
/* Call cb for each entry of the directory at path. 0 or -1. */
int fs_bcachefs_readdir(fs_bcachefs_fs *fs, const char *path, fs_bcachefs_dirent_cb cb, void *ctx);
/* Copy up to length bytes from offset into buf. Bytes copied, or -1.
 * A symlink's "contents" are its target. */
int64_t fs_bcachefs_read_file(fs_bcachefs_fs *fs, const char *path, void *buf,
                              uint64_t offset, uint64_t length);

#ifdef __cplusplus
}
#endif
#endif /* FS_BCACHEFS_H */
