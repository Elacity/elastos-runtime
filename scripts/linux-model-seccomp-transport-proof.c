// Linux SEC1 transport design proof, not an installed model-provider test.
// Build: gcc -O2 -Wall -Wextra -Werror -std=c11 this-file.c -o proof
// Runtime owns the engine listener and broker. The provider receives fd 3 only.
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

struct provider_report {
    int pid;
    int ppid;
    int seccomp_mode;
    int no_new_privs;
    int only_broker_fd;
    int ipv4_tcp_denied;
    int ipv4_udp_denied;
    int ipv6_tcp_denied;
    int unix_socket_denied;
    int socketpair_denied;
    int connect_denied;
    int bind_denied;
    int listen_denied;
    int accept_denied;
    int sendto_denied;
    int broker_roundtrip;
};

struct engine_report {
    int pid;
    int ppid;
    int accepted;
};

static void exact_write(int fd, const void *bytes, size_t length) {
    const char *next = bytes;
    while (length) {
        ssize_t n = write(fd, next, length);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) _exit(91);
        next += n;
        length -= (size_t)n;
    }
}

static void exact_read(int fd, void *bytes, size_t length) {
    char *next = bytes;
    while (length) {
        ssize_t n = read(fd, next, length);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) _exit(92);
        next += n;
        length -= (size_t)n;
    }
}

#define DENY(number) \
    BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, (number), 0, 1), \
    BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM)

static int install_provider_filter(void) {
    struct sock_filter code[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, arch)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JGE | BPF_K, 0x40000000, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
        DENY(__NR_socket),
        DENY(__NR_socketpair),
        DENY(__NR_connect),
        DENY(__NR_bind),
        DENY(__NR_listen),
        DENY(__NR_accept),
        DENY(__NR_accept4),
        DENY(__NR_sendto),
        DENY(__NR_sendmsg),
        DENY(__NR_sendmmsg),
        DENY(__NR_recvmsg),
        DENY(__NR_recvmmsg),
        DENY(__NR_io_uring_setup),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program = {
        .len = (unsigned short)(sizeof(code) / sizeof(code[0])),
        .filter = code,
    };
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0) return -1;
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program);
}

static int denied_socket(int domain, int type) {
    errno = 0;
    int fd = socket(domain, type, 0);
    if (fd >= 0) close(fd);
    return fd < 0 && errno == EPERM;
}

static int only_broker_fd(int broker) {
    if (broker != 3) return 0;
    for (int fd = 0; fd <= 3; fd++) {
        if (fcntl(fd, F_GETFD) < 0) return 0;
    }
    for (int fd = 4; fd < 64; fd++) {
        errno = 0;
        if (fcntl(fd, F_GETFD) >= 0 || errno != EBADF) return 0;
    }
    return 1;
}

static void provider_probe(int broker, int descendant) {
    struct provider_report r = {
        .pid = (int)getpid(),
        .ppid = (int)getppid(),
        .seccomp_mode = prctl(PR_GET_SECCOMP, 0, 0, 0, 0),
        .no_new_privs = prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0),
        .only_broker_fd = only_broker_fd(broker),
    };
    r.ipv4_tcp_denied = denied_socket(AF_INET, SOCK_STREAM);
    r.ipv4_udp_denied = denied_socket(AF_INET, SOCK_DGRAM);
    r.ipv6_tcp_denied = denied_socket(AF_INET6, SOCK_STREAM);
    r.unix_socket_denied = denied_socket(AF_UNIX, SOCK_STREAM);
    int pair[2] = {-1, -1};
    errno = 0;
    r.socketpair_denied =
        socketpair(AF_UNIX, SOCK_STREAM, 0, pair) < 0 && errno == EPERM;
    if (pair[0] >= 0) {
        close(pair[0]);
        close(pair[1]);
    }
    struct sockaddr_in local = {
        .sin_family = AF_INET,
        .sin_port = htons(9),
        .sin_addr.s_addr = htonl(INADDR_LOOPBACK),
    };
    errno = 0;
    r.connect_denied =
        connect(broker, (struct sockaddr *)&local, sizeof(local)) < 0 &&
        errno == EPERM;
    errno = 0;
    r.bind_denied =
        bind(broker, (struct sockaddr *)&local, sizeof(local)) < 0 &&
        errno == EPERM;
    errno = 0;
    r.listen_denied = listen(broker, 1) < 0 && errno == EPERM;
    errno = 0;
    r.accept_denied = accept(broker, NULL, NULL) < 0 && errno == EPERM;
    errno = 0;
    r.sendto_denied =
        sendto(broker, "x", 1, 0, (struct sockaddr *)&local, sizeof(local)) < 0 &&
        errno == EPERM;
    const char *request = descendant ? "infer:descendant\n" : "infer:child\n";
    const char *answer = descendant ? "local:descendant\n" : "local:child\n";
    char response[32] = {0};
    exact_write(broker, request, strlen(request));
    exact_read(broker, response, strlen(answer));
    r.broker_roundtrip = memcmp(response, answer, strlen(answer)) == 0;
    exact_write(broker, &r, sizeof(r));
}

static int report_passed(const struct provider_report *r) {
    return r->seccomp_mode == 2 && r->no_new_privs == 1 &&
           r->only_broker_fd && r->ipv4_tcp_denied && r->ipv4_udp_denied &&
           r->ipv6_tcp_denied && r->unix_socket_denied &&
           r->socketpair_denied && r->connect_denied && r->bind_denied &&
           r->listen_denied && r->accept_denied && r->sendto_denied &&
           r->broker_roundtrip;
}

static void engine_serve(int listener, int runtime_pid) {
    int client = accept(listener, NULL, NULL);
    if (client < 0) _exit(11);
    close(listener);
    struct engine_report report = {
        .pid = (int)getpid(),
        .ppid = (int)getppid(),
        .accepted = 1,
    };
    if (report.ppid != runtime_pid) _exit(12);
    exact_write(client, &report, sizeof(report));
    const char *requests[] = {"infer:child\n", "infer:descendant\n"};
    const char *answers[] = {"local:child\n", "local:descendant\n"};
    for (size_t i = 0; i < 2; i++) {
        char received[32] = {0};
        exact_read(client, received, strlen(requests[i]));
        if (memcmp(received, requests[i], strlen(requests[i])) != 0) _exit(13);
        exact_write(client, answers[i], strlen(answers[i]));
    }
    close(client);
    _exit(0);
}

static void runtime_relay(int broker, int engine, const char *request,
                          const char *answer, struct provider_report *report) {
    char received[32] = {0};
    exact_read(broker, received, strlen(request));
    if (memcmp(received, request, strlen(request)) != 0) _exit(21);
    exact_write(engine, request, strlen(request));
    exact_read(engine, received, strlen(answer));
    if (memcmp(received, answer, strlen(answer)) != 0) _exit(22);
    exact_write(broker, answer, strlen(answer));
    exact_read(broker, report, sizeof(*report));
}

static void print_provider(const char *name, const struct provider_report *r) {
    printf("\"%s\":{\"pid\":%d,\"seccomp_mode\":%d,\"no_new_privs\":%d,"
           "\"only_broker_fd\":%s,\"ipv4_tcp_denied\":%s,"
           "\"ipv4_udp_denied\":%s,\"ipv6_tcp_denied\":%s,"
           "\"unix_socket_denied\":%s,\"socketpair_denied\":%s,"
           "\"connect_denied\":%s,\"bind_denied\":%s,\"listen_denied\":%s,"
           "\"accept_denied\":%s,\"sendto_denied\":%s,"
           "\"broker_roundtrip\":%s}",
           name, r->pid, r->seccomp_mode, r->no_new_privs,
           r->only_broker_fd ? "true" : "false",
           r->ipv4_tcp_denied ? "true" : "false",
           r->ipv4_udp_denied ? "true" : "false",
           r->ipv6_tcp_denied ? "true" : "false",
           r->unix_socket_denied ? "true" : "false",
           r->socketpair_denied ? "true" : "false",
           r->connect_denied ? "true" : "false",
           r->bind_denied ? "true" : "false",
           r->listen_denied ? "true" : "false",
           r->accept_denied ? "true" : "false",
           r->sendto_denied ? "true" : "false",
           r->broker_roundtrip ? "true" : "false");
}

int main(int argc, char **argv) {
    if (argc == 3 && strcmp(argv[1], "--provider") == 0) {
        int broker = atoi(argv[2]);
        if (broker != 3) return 31;
        provider_probe(broker, 0);
        pid_t descendant = fork();
        if (descendant < 0) return 32;
        if (descendant == 0) {
            provider_probe(broker, 1);
            _exit(0);
        }
        int status = 0;
        if (waitpid(descendant, &status, 0) != descendant ||
            !WIFEXITED(status) || WEXITSTATUS(status) != 0) return 33;
        return 0;
    }
    if (argc != 1) return 34;
    int broker[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, broker) != 0) return 1;
    int listener = socket(AF_UNIX, SOCK_STREAM, 0);
    if (listener < 0) return 2;
    struct sockaddr_un address = {.sun_family = AF_UNIX};
    int length = snprintf(address.sun_path + 1, sizeof(address.sun_path) - 1,
                          "elastos-sec1-%ld", (long)getpid());
    if (length < 0 || (size_t)length >= sizeof(address.sun_path) - 1) return 3;
    socklen_t address_length =
        (socklen_t)(offsetof(struct sockaddr_un, sun_path) + 1 + length);
    if (bind(listener, (struct sockaddr *)&address, address_length) != 0 ||
        listen(listener, 1) != 0) return 4;
    pid_t runtime_pid = getpid();
    pid_t engine_pid = fork();
    if (engine_pid < 0) return 5;
    if (engine_pid == 0) {
        close(broker[0]);
        close(broker[1]);
        engine_serve(listener, (int)runtime_pid);
    }
    int engine = socket(AF_UNIX, SOCK_STREAM, 0);
    if (engine < 0 ||
        connect(engine, (struct sockaddr *)&address, address_length) != 0)
        return 6;
    close(listener);
    struct engine_report engine_result = {0};
    exact_read(engine, &engine_result, sizeof(engine_result));
    pid_t provider_pid = fork();
    if (provider_pid < 0) return 7;
    if (provider_pid == 0) {
        close(broker[0]);
        close(engine);
        int nullfd = open("/dev/null", O_RDWR);
        if (nullfd < 0) _exit(41);
        for (int fd = 0; fd <= 2; fd++)
            if (dup2(nullfd, fd) < 0) _exit(42);
        if (nullfd > 2) close(nullfd);
        if (broker[1] != 3) {
            if (dup2(broker[1], 3) < 0) _exit(43);
            close(broker[1]);
        }
        if (syscall(SYS_close_range, 4u, ~0u, 0u) != 0) _exit(44);
        if (install_provider_filter() != 0) _exit(45);
        char *args[] = {argv[0], "--provider", "3", NULL};
        execv(argv[0], args);
        _exit(46);
    }
    close(broker[1]);
    struct provider_report child = {0};
    struct provider_report descendant = {0};
    runtime_relay(broker[0], engine, "infer:child\n", "local:child\n", &child);
    runtime_relay(broker[0], engine, "infer:descendant\n",
                  "local:descendant\n", &descendant);
    int provider_status = 0;
    int engine_status = 0;
    if (waitpid(provider_pid, &provider_status, 0) != provider_pid ||
        waitpid(engine_pid, &engine_status, 0) != engine_pid) return 8;
    int passed = engine_result.pid == engine_pid &&
                 engine_result.ppid == runtime_pid && engine_result.accepted &&
                 WIFEXITED(engine_status) && WEXITSTATUS(engine_status) == 0 &&
                 WIFEXITED(provider_status) && WEXITSTATUS(provider_status) == 0 &&
                 child.pid == provider_pid && descendant.ppid == provider_pid &&
                 report_passed(&child) && report_passed(&descendant);
    printf("{\"schema\":\"elastos.model.linux-seccomp-transport-proof/v1\","
           "\"runtime_pid\":%d,\"engine_pid\":%d,\"engine_accepted\":%s,",
           (int)runtime_pid, (int)engine_pid,
           engine_result.accepted ? "true" : "false");
    print_provider("provider", &child);
    printf(",");
    print_provider("descendant", &descendant);
    printf(",\"result\":\"%s\"}\n", passed ? "pass" : "fail");
    close(broker[0]);
    close(engine);
    return passed ? 0 : 9;
}
