FROM node:22-bookworm-slim AS web
WORKDIR /src
ENV PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
RUN corepack enable && corepack prepare pnpm@9.15.0 --activate
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY packages ./packages
COPY apps/studio-web ./apps/studio-web
RUN pnpm install --frozen-lockfile --filter @layrs/studio-web...
RUN pnpm --filter @layrs/studio-web build

FROM caddy:2.10.2-alpine
COPY --from=web /src/apps/studio-web/dist /srv/layrs
COPY deploy/selfhost/Caddyfile /etc/caddy/Caddyfile
