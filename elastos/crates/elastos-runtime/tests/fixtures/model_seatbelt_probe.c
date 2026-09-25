#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

static int opened(const char *path, int flags) {
    int fd = open(path, flags);
    if (fd < 0) return errno;
    close(fd);
    return 0;
}

static int inet_result(const char *ip, unsigned short port) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in addr = {.sin_family = AF_INET, .sin_port = htons(port)};
    inet_pton(AF_INET, ip, &addr.sin_addr);
    int result = connect(fd, (struct sockaddr *)&addr, sizeof(addr)) == 0 ? 0 : errno;
    close(fd);
    return result;
}

static int unix_result(const char *path) {
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un addr = {.sun_family = AF_UNIX};
    snprintf(addr.sun_path, sizeof(addr.sun_path), "%s", path);
    int result = connect(fd, (struct sockaddr *)&addr, sizeof(addr)) == 0 ? 0 : errno;
    close(fd);
    return result;
}

int main(void) {
    int inherited_writable_regular_fds = 0;
    for (int fd = 3; fd < 256; fd++) {
        struct stat metadata;
        int flags = fcntl(fd, F_GETFL);
        if (flags >= 0 && fstat(fd, &metadata) == 0 && S_ISREG(metadata.st_mode) &&
            (flags & O_ACCMODE) != O_RDONLY) inherited_writable_regular_fds++;
    }
    char line[8192];
    while (fgets(line, sizeof(line), stdin)) {
        if (strstr(line, "\"op\":\"init\"")) {
            puts("{\"status\":\"ok\"}"); fflush(stdout);
            continue;
        }
        if (strstr(line, "\"op\":\"shutdown\"")) {
            puts("{\"status\":\"ok\"}"); fflush(stdout);
            return 0;
        }
        char *field = strstr(line, "\"path\":\"");
        if (!field) return 2;
        field += strlen("\"path\":\"");
        char *end = strchr(field, '"');
        if (!end) return 3;
        *end = 0;
        char *save = NULL;
        char *selected = strtok_r(field, "|", &save);
        char *port_text = strtok_r(NULL, "|", &save);
        char *other = strtok_r(NULL, "|", &save);
        char *model = strtok_r(NULL, "|", &save);
        char *canary = strtok_r(NULL, "|", &save);
        char *journal = strtok_r(NULL, "|", &save);
        char *base = strtok_r(NULL, "|", &save);
        if (!selected || !port_text || !other || !model || !canary || !journal || !base) return 4;
        char resolved[4096];
        int base_canonical = realpath(base, resolved) ? 0 : errno;
        unsigned short port = (unsigned short)strtoul(port_text, NULL, 10);
        int external = inet_result("203.0.113.1", 443);
        int unrelated = inet_result("127.0.0.1", port);
        int other_socket = unix_result(other);
        int selected_socket = unix_result(selected);
        int system_read = opened("/usr/bin", O_RDONLY);
        int canary_read = opened(canary, O_RDONLY);
        int model_read = opened(model, O_RDONLY);
        int model_write = opened(model, O_WRONLY | O_TRUNC);
        char alias[4096];
        snprintf(alias, sizeof(alias), "%s/probe-link", journal);
        int model_link = link(model, alias) == 0 ? 0 : errno;
        snprintf(alias, sizeof(alias), "%s/probe-symlink", journal);
        int symlink_create = symlink(model, alias) == 0 ? 0 : errno;
        int alias_write = opened(alias, O_WRONLY | O_TRUNC);
        snprintf(alias, sizeof(alias), "%s/probe-write", journal);
        int journal_write = opened(alias, O_WRONLY | O_CREAT | O_EXCL);
        int pipes[2];
        if (pipe(pipes) != 0) return 5;
        pid_t pid = fork();
        if (pid < 0) return 6;
        if (pid == 0) {
            close(pipes[0]);
            char child_alias[4096];
            snprintf(child_alias, sizeof(child_alias), "%s/child-link", journal);
            int child_link = link(model, child_alias) == 0 ? 0 : errno;
            snprintf(child_alias, sizeof(child_alias), "%s/child-moved", journal);
            int child_rename = rename(model, child_alias) == 0 ? 0 : errno;
            int descendant[9] = {inet_result("203.0.113.1", 443),
                                 inet_result("127.0.0.1", port), unix_result(other),
                                 unix_result(selected), opened("/usr/bin", O_RDONLY),
                                 opened(canary, O_RDONLY),
                                 opened(model, O_WRONLY | O_TRUNC), child_link,
                                 child_rename};
            int wrote = (int)write(pipes[1], descendant, sizeof(descendant));
            _exit(wrote == sizeof(descendant) ? 0 : 7);
        }
        close(pipes[1]);
        int descendant[9] = {0};
        if (read(pipes[0], descendant, sizeof(descendant)) != sizeof(descendant)) return 8;
        close(pipes[0]);
        int status;
        if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status)) return 9;
        snprintf(alias, sizeof(alias), "%s/probe-moved", journal);
        int model_rename = rename(model, alias) == 0 ? 0 : errno;
        printf("{\"status\":\"ok\",\"data\":{\"direct_errno\":%d,\"descendant_errno\":%d,"
               "\"unrelated_errno\":%d,\"descendant_unrelated_errno\":%d,"
               "\"other_socket_errno\":%d,\"descendant_other_socket_errno\":%d,"
               "\"selected_socket_errno\":%d,"
               "\"descendant_selected_socket_errno\":%d,\"system_read_errno\":%d,"
               "\"descendant_system_read_errno\":%d,\"canary_read_errno\":%d,"
               "\"descendant_canary_read_errno\":%d,\"model_read_errno\":%d,"
               "\"model_write_errno\":%d,\"descendant_model_write_errno\":%d,"
               "\"model_link_errno\":%d,\"descendant_model_link_errno\":%d,"
               "\"model_rename_errno\":%d,\"descendant_model_rename_errno\":%d,"
               "\"journal_alias_create_errno\":%d,\"journal_alias_write_errno\":%d,"
               "\"journal_write_errno\":%d,\"base_canonical_errno\":%d,"
               "\"inherited_writable_regular_fds\":%d}}\n",
               external, descendant[0], unrelated, descendant[1], other_socket,
               descendant[2], selected_socket, descendant[3], system_read,
               descendant[4], canary_read, descendant[5], model_read, model_write,
               descendant[6], model_link, descendant[7], model_rename, descendant[8],
               symlink_create, alias_write, journal_write, base_canonical,
               inherited_writable_regular_fds);
        fflush(stdout);
    }
    return 0;
}
