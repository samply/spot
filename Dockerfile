# This assumes binaries are present, see COPY directive.
FROM alpine AS chmodder
ARG TARGETARCH
COPY /artifacts/binaries-$TARGETARCH/spot /app/
RUN chmod +x /app/*

FROM gcr.io/distroless/cc-debian12
COPY --from=chmodder /app/* /usr/local/bin/
ENTRYPOINT [ "/usr/local/bin/spot" ]
