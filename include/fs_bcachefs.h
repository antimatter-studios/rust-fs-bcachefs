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

/* ---- At the level of the family's other drivers ---------------------- */

/* The last failure on this thread, as fs_core_last_error_message(). Never NULL. */
const char *fs_bcachefs_last_error(void);

/* Mount through an fs_core device handle (fs_core_file_open,
 * fs_core_device_from_callbacks, a slice). The handle stays the caller's to
 * close; the filesystem keeps its own reference. NULL on failure. */
fs_bcachefs_fs *fs_bcachefs_mount_with_fs_core_device(const struct FsCoreDevice *dev);

typedef struct {
    uint32_t block_size;      /* bytes */
    uint32_t btree_node_size; /* bytes */
    uint32_t nr_devices;
    uint16_t version_major;
    uint16_t version_minor;
    uint8_t  uuid[16];        /* the external UUID */
    char     label[33];       /* NUL-terminated */
    uint8_t  clean;           /* 1: shut down cleanly; 0: read through a journal replay */
} fs_bcachefs_volume_info_t;

/* The volume's identity and geometry. 0 or -1. */
int fs_bcachefs_get_volume_info(fs_bcachefs_fs *fs, fs_bcachefs_volume_info_t *out);

/* Stat an inode by number. 0 or -1. */
int fs_bcachefs_stat_ino(fs_bcachefs_fs *fs, uint64_t ino, fs_bcachefs_attr_t *out);

typedef struct fs_bcachefs_dir_iter fs_bcachefs_dir_iter;

typedef struct {
    uint64_t ino;
    uint8_t  d_type;          /* DT_* numbering */
    uint16_t name_len;
    char     name[256];       /* NUL-terminated */
} fs_bcachefs_dirent_t;

/* List a directory: open, then next until NULL, then close. An entry is
 * valid until the next call on its iterator. */
fs_bcachefs_dir_iter *fs_bcachefs_dir_open(fs_bcachefs_fs *fs, const char *path);
const fs_bcachefs_dirent_t *fs_bcachefs_dir_next(fs_bcachefs_dir_iter *it);
void fs_bcachefs_dir_close(fs_bcachefs_dir_iter *it);

/* A symlink's target (not NUL-terminated). With len 0, its length. Returns
 * the length, or -1 (not a symlink, or buf too small). */
int64_t fs_bcachefs_readlink(fs_bcachefs_fs *fs, const char *path, void *buf, uint64_t len);

/* Extended attributes, as listxattr(2)/getxattr(2) lay them out: names are
 * NUL-terminated and carry their namespace ("user.x"). With len 0, the
 * size needed. Returns the size, or -1. */
int64_t fs_bcachefs_listxattr(fs_bcachefs_fs *fs, const char *path, void *buf, uint64_t len);
int64_t fs_bcachefs_getxattr(fs_bcachefs_fs *fs, const char *path, const char *name,
                             void *buf, uint64_t len);

/* WRITING (#103): only in a library built with the `write` feature.
 * Experimental. An image opened with fs_bcachefs_mount_rw is changed in
 * place, one transaction per call: when a call returns 0 the image holds
 * its result, and fs_bcachefs_rw_close has nothing left to write. A
 * filesystem that was not cleanly unmounted is refused at open. Paths are
 * absolute. Every call returns 0 or a negative errno: -ENOENT (no such
 * path), -ENOTSUP (not something the writer does), -ENOSPC (no free
 * space), -ENOTEMPTY, -EEXIST, -EINVAL (a NULL or malformed argument),
 * -EIO (anything else); the reason is in fs_bcachefs_last_error(). */
typedef struct fs_bcachefs_rw fs_bcachefs_rw;

fs_bcachefs_rw *fs_bcachefs_mount_rw(const char *path);
void fs_bcachefs_rw_close(fs_bcachefs_rw *fs);

int fs_bcachefs_create(fs_bcachefs_rw *fs, const char *path, const void *data, uint64_t len,
                       uint32_t mode);
int fs_bcachefs_mkdir(fs_bcachefs_rw *fs, const char *path, uint32_t mode);
int fs_bcachefs_unlink(fs_bcachefs_rw *fs, const char *path);
int fs_bcachefs_rmdir(fs_bcachefs_rw *fs, const char *path);
/* A file already at `to` is replaced; a directory may move to another. */
int fs_bcachefs_rename(fs_bcachefs_rw *fs, const char *from, const char *to);
int fs_bcachefs_symlink(fs_bcachefs_rw *fs, const char *path, const char *target);
int fs_bcachefs_link(fs_bcachefs_rw *fs, const char *existing, const char *path);
/* Replace a file's whole contents. */
int fs_bcachefs_write_file(fs_bcachefs_rw *fs, const char *path, const void *data, uint64_t len);
int fs_bcachefs_chmod(fs_bcachefs_rw *fs, const char *path, uint32_t mode);
int fs_bcachefs_chown(fs_bcachefs_rw *fs, const char *path, uint32_t uid, uint32_t gid);
int fs_bcachefs_setxattr(fs_bcachefs_rw *fs, const char *path, const char *name,
                         const void *value, uint64_t len);
int fs_bcachefs_removexattr(fs_bcachefs_rw *fs, const char *path, const char *name);

#ifdef __cplusplus
}
#endif
#endif /* FS_BCACHEFS_H */
