/* ficlone.c SRC DST -- DST becomes a reflink of SRC through the FICLONE
 * ioctl, so the reference kernel module shares SRC's data instead of
 * copying it (#7). Built static in kernel-oracle.sh's container and run by
 * kernel-oracle-init.sh, whose busybox cp cannot reflink. */
#include <fcntl.h>
#include <linux/fs.h>
#include <stdio.h>
#include <sys/ioctl.h>
#include <unistd.h>

int main(int argc, char **argv)
{
    if (argc != 3) {
        fprintf(stderr, "usage: ficlone SRC DST\n");
        return 2;
    }
    int src = open(argv[1], O_RDONLY);
    int dst = open(argv[2], O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (src < 0 || dst < 0) {
        perror("open");
        return 1;
    }
    if (ioctl(dst, FICLONE, src) < 0) {
        perror("FICLONE");
        return 1;
    }
    return close(dst) < 0;
}
