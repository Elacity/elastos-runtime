#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

int main(void) {
    char line[8192];
    while (fgets(line, sizeof(line), stdin)) {
        if (strstr(line, "\"op\":\"init\"") ||
            strstr(line, "\"op\":\"runs_create\"") ||
            strstr(line, "\"op\":\"shutdown\"")) {
            puts("{\"status\":\"ok\"}"); fflush(stdout);
            if (strstr(line, "\"op\":\"shutdown\"")) return 0;
            continue;
        }
        char *field = strstr(line, "\"path\":\"");
        if (!field) return 2;
        field += strlen("\"path\":\"");
        char *end = strchr(field, '"');
        if (!end) return 3;
        *end = 0;
        char *save = NULL;
        char *kind = strtok_r(field, "|", &save);
        char *socket_path = strtok_r(NULL, "|", &save);
        char *offer = strtok_r(NULL, "|", &save);
        char *run = strtok_r(NULL, "|", &save);
        char *request = strtok_r(NULL, "|", &save);
        if (!kind || !socket_path || !offer || !run || !request) return 4;
        const char *prompt = strcmp(kind, "altered") == 0 ? "changed text" : "authorized text";
        const char *cap = strcmp(kind, "missing-cap") == 0 ? "" :
                          strcmp(kind, "raised-cap") == 0 ? "\"max_tokens\":33," : "\"max_tokens\":32,";
        char body[1024];
        int body_len = snprintf(body, sizeof(body),
                                "{\"model\":\"fixture/model\",\"stream\":true,%s"
                                "\"messages\":[{\"role\":\"user\",\"content\":\"%s\"}]}",
                                cap, prompt);
        if (body_len < 0 || (size_t)body_len >= sizeof(body)) return 5;
        char message[4096];
        int length = snprintf(message, sizeof(message),
                              "POST /v1/hosted-effect HTTP/1.1\r\nHost: runtime.invalid\r\n"
                              "Content-Type: application/json\r\nContent-Length: %d\r\n"
                              "X-Elastos-Offer-Id: %s\r\nX-Elastos-Effect: text\r\n"
                              "X-Elastos-Run-Id: %s\r\nX-Elastos-Request-Id: %s\r\n\r\n%s",
                              body_len, offer, run, request, body);
        if (length < 0 || (size_t)length >= sizeof(message)) return 6;
        int client = socket(AF_UNIX, SOCK_STREAM, 0);
        struct sockaddr_un address = {.sun_family = AF_UNIX};
        snprintf(address.sun_path, sizeof(address.sun_path), "%s", socket_path);
        if (connect(client, (struct sockaddr *)&address, sizeof(address)) != 0) return 7;
        if (write(client, message, length) != length) return 8;
        char reply[256];
        ssize_t count = read(client, reply, sizeof(reply) - 1);
        close(client);
        if (count < 0) return 9;
        reply[count] = 0;
        char *first_line = strstr(reply, "\r\n");
        if (!first_line) return 10;
        *first_line = 0;
        printf("{\"status\":\"ok\",\"data\":{\"http_status\":\"%s\"}}\n", reply);
        fflush(stdout);
    }
    return 0;
}
