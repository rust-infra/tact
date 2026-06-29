FROM alpine:3.20

RUN apk add --no-cache bash curl jq git openssh-client

WORKDIR /work
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]
