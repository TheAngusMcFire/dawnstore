#!/bin/env nu

docker compose down --volumes; docker compose up -d
cd ../dawnstore-core/
sqlx migrate run
cd -
