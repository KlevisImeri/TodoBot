FROM fedora:latest

RUN dnf install -y \
      ca-certificates tzdata shadow-utils util-linux && \
    dnf clean all

RUN useradd --create-home --uid 10001 todobot

COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

ENV TZ="Europe/Zurich"
ENV RUST_LOG="info"

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
