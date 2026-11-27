#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static int read_all(void *destination, size_t count) {
    char *bytes = destination;
    while (count > 0) {
        ssize_t got = read(STDIN_FILENO, bytes, count);
        if (got <= 0)
            return 0;
        bytes += got;
        count -= (size_t)got;
    }
    return 1;
}

static int write_all(const void *source, size_t count) {
    const char *bytes = source;
    while (count > 0) {
        ssize_t sent = write(STDOUT_FILENO, bytes, count);
        if (sent <= 0)
            return 0;
        bytes += sent;
        count -= (size_t)sent;
    }
    return 1;
}

int main(void) {
    struct pollfd descriptors[] = {{STDIN_FILENO, 0, 0}, {STDOUT_FILENO, 0, 0},
                                   {STDERR_FILENO, 0, 0}};
    if (poll(descriptors, 3, 0) < 0)
        return 9;
    unsigned char size[4];
    char request[2048];
    if (!read_all(size, sizeof(size)))
        return 10;
    size_t length = ((size_t)size[0] << 24) | ((size_t)size[1] << 16) |
                    ((size_t)size[2] << 8) | (size_t)size[3];
    if (length >= sizeof(request) || !read_all(request, length))
        return 11;
    request[length] = '\0';
    char *target = strstr(request, "\"target\":\"");
    char *operation = strstr(request, "\"operation\":{");
    if (!target || !operation)
        return 12;
    target += strlen("\"target\":\"");
    char *end = strchr(target, '"');
    if (!end || end - target > 128)
        return 13;

    char path[256];
    snprintf(path, sizeof(path), "/tmp/%.*s/history.sqlite", (int)(end - target), target);
    if (open(path, O_RDONLY) != -1 || errno != EPERM)
        return 20;
    snprintf(path, sizeof(path), "/tmp/%.*s/unrelated-user-file", (int)(end - target), target);
    if (open(path, O_RDONLY) != -1 || errno != EPERM)
        return 21;
    if (read(199, path, sizeof(path)) != -1 || errno != EBADF)
        return 22;
    if (getenv("AGENTURM_CONNECTOR_TEST_CREDENTIAL"))
        return 23;
    if (socket(AF_INET, SOCK_STREAM, 0) != -1 || errno != EPERM)
        return 24;
    if (socket(AF_UNIX, SOCK_STREAM, 0) != -1 || errno != EPERM)
        return 25;
    if (fork() != -1 || errno != EPERM)
        return 26;
    char *args[] = { "/bin/true", NULL };
    char *environment[] = { NULL };
    execve(args[0], args, environment);
    if (errno != EACCES)
        return 27;

    const char *outcome;
    if (strstr(operation, "\"kind\":\"poll\"")) {
        outcome = "\"outcome\":{\"kind\":\"batch\",\"batch\":"
                  "{\"next_checkpoint\":\"sandbox-1\",\"records\":["
                  "{\"record_id\":\"sandbox-record\",\"revision\":0,"
                  "\"observed_at_unix_ns\":1,\"payload\":"
                  "{\"session_id\":\"sandbox-session\",\"state\":\"completed\"}}]}}}";
    } else if (strstr(operation, "\"kind\":\"ack\"")) {
        outcome = "\"outcome\":{\"kind\":\"ack\",\"checkpoint\":\"sandbox-1\"}}";
    } else if (strstr(operation, "\"kind\":\"deliver\"") ||
               strstr(operation, "\"kind\":\"lookup\"")) {
        if (strstr(request, "\"credential\"") || strstr(request, "\"host_transport\"") ||
            strstr(request, "connector-fixture-secret") ||
            !strstr(operation, "\"action\":\"write_status\""))
            return 29;
        outcome = "\"outcome\":{\"kind\":\"forward\"}}";
    } else {
        return 30;
    }
    length = (size_t)(operation - request);
    size_t total = length + strlen(outcome);
    size[0] = total >> 24;
    size[1] = total >> 16;
    size[2] = total >> 8;
    size[3] = total;
    return !(write_all(size, sizeof(size)) && write_all(request, length) &&
             write_all(outcome, strlen(outcome)));
}
