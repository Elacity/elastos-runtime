#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

static int nibble(char value) {
    if (value >= '0' && value <= '9') return value - '0';
    if (value >= 'a' && value <= 'f') return value - 'a' + 10;
    return -1;
}

int main(void) {
    char line[32768];
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
        char *socket_path = strtok_r(field, "|", &save);
        char *offer = strtok_r(NULL, "|", &save);
        char *run = strtok_r(NULL, "|", &save);
        char *request = strtok_r(NULL, "|", &save);
        char *encoded = strtok_r(NULL, "|", &save);
        if (!socket_path || !offer || !run || !request || !encoded) return 4;
        char body[8192];
        size_t encoded_len = strlen(encoded);
        size_t body_len = encoded_len / 2;
        if (encoded_len % 2 || body_len >= sizeof(body)) return 5;
        for (size_t i = 0; i < body_len; i++) {
            int high = nibble(encoded[2 * i]), low = nibble(encoded[2 * i + 1]);
            if (high < 0 || low < 0) return 5;
            body[i] = (char)(16 * high + low);
        }
        body[body_len] = 0;
        char message[16384];
        int length = snprintf(message, sizeof(message),
                              "POST /v1/hosted-effect HTTP/1.1\r\nHost: runtime.invalid\r\n"
                              "Content-Type: application/json\r\nContent-Length: %zu\r\n"
                              "X-Elastos-Offer-Id: %s\r\nX-Elastos-Effect: text\r\n"
                              "X-Elastos-Run-Id: %s\r\nX-Elastos-Request-Id: %s\r\n\r\n%s",
                              body_len, offer, run, request, body);
        if (length < 0 || (size_t)length >= sizeof(message)) return 6;
        int client = socket(AF_UNIX, SOCK_STREAM, 0);
        struct sockaddr_un address = {.sun_family = AF_UNIX};
        if (strlen(socket_path) >= sizeof(address.sun_path)) return 7;
        memcpy(address.sun_path, socket_path, strlen(socket_path) + 1);
        if (connect(client, (struct sockaddr *)&address, sizeof(address)) != 0) return 7;
        size_t sent = 0;
        while (sent < (size_t)length) {
            ssize_t count = write(client, message + sent, (size_t)length - sent);
            if (count <= 0) return 8;
            sent += (size_t)count;
        }
        char reply[256];
        ssize_t count = read(client, reply, sizeof(reply) - 1);
        close(client);
        if (count <= 0) return 9;
        reply[count] = 0;
        char *first_line = strstr(reply, "\r\n");
        if (!first_line) return 10;
        *first_line = 0;
        printf("{\"status\":\"ok\",\"data\":{\"http_status\":\"%s\"}}\n", reply);
        fflush(stdout);
    }
    return 0;
}
