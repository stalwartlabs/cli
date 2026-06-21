FROM gcr.io/distroless/static-debian12:nonroot
ARG TARGETARCH
COPY bin/${TARGETARCH}/stalwart-cli /usr/local/bin/stalwart-cli
ENTRYPOINT ["/usr/local/bin/stalwart-cli"]
