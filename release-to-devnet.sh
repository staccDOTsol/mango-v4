#!/usr/bin/env bash

set -ex pipefail

WALLET_WITH_FUNDS=~/7i.json
PROGRAM_ID=5JfWyyooqZbKpA9ZbZSrbPke4TKyxV2mo5wcLEptQ5NG

# build program, 
anchor build -- --features enable-gpl

# patch types, which we want in rust, but anchor client doesn't support
./idl-fixup.sh

# update types in ts client package
cp -v ./target/types/mango_v4.ts ./ts/client/src/mango_v4.ts

(cd ./ts/client && yarn tsc)

# publish program
solana --url https://rpcrpc.rpc.devnet program deploy --program-id $PROGRAM_ID  \
    -k $WALLET_WITH_FUNDS target/deploy/mango_v4.so --skip-fee-check 

# publish idl
anchor idl upgrade --provider.cluster https://rpcrpc.rpc.devnet --provider.wallet $WALLET_WITH_FUNDS \
    --filepath target/idl/mango_v4_no_docs.json $PROGRAM_ID
