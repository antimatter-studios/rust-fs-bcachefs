/* prealloc.c PATH -- preallocate the first MiB of PATH with fallocate and
 * write 4 KiB of 'w' at 64 KiB, so the rest of that MiB is space the file
 * holds but never wrote (#158). Built static in kernel-oracle.sh's
 * container and run by kernel-oracle-init.sh, whose busybox has no
 * fallocate. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv)
{
    if (argc != 2) {
        fprintf(stderr, "usage: prealloc PATH\n");
        return 2;
    }
    int fd = open(argv[1], O_RDWR);
    if (fd < 0) {
        perror("open");
        return 1;
    }
    if (fallocate(fd, 0, 0, 1 << 20) != 0) {
        perror("fallocate");
        return 1;
    }
    char buf[4096];
    memset(buf, 'w', sizeof buf);
    if (pwrite(fd, buf, sizeof buf, 65536) != (ssize_t)sizeof buf) {
        perror("pwrite");
        return 1;
    }
    if (fsync(fd) != 0) {
        perror("fsync");
        return 1;
    }
    return close(fd) != 0;
}
