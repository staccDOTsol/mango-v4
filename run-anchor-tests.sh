#!/bin/bash

# WALLET_WITH_FUNDS=~/.config/solana/mango-devnet.json
# PROGRAM_ID=5JfWyyooqZbKpA9ZbZSrbPke4TKyxV2mo5wcLEptQ5NG

anchor build -- --features enable-gpl
./idl-fixup.sh
RUST_BACKTRACE=full anchor test --skip-build
