#![cfg(all(feature = "test-bpf"))]

use fixed::types::I80F48;
use mango_setup::*;
use mango_v4::{error::MangoError, state::*};
use program_test::*;
use solana_program_test::*;
use solana_sdk::transport::TransportError;

mod program_test;

#[tokio::test]
async fn test_perp_settle_pnl() -> Result<(), TransportError> {
    let context = TestContext::new().await;
    let solana = &context.solana.clone();

    let admin = TestKeypair::new();
    let owner = context.users[0].key;
    let payer = context.users[1].key;
    let mints = &context.mints[0..=2];

    let initial_token_deposit = 10_000;

    //
    // SETUP: Create a group and an account
    //

    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;

    let settler =
        create_funded_account(&solana, group, owner, 251, &context.users[1], &[], 0, 0).await;
    let settler_owner = owner.clone();

    let account_0 = create_funded_account(
        &solana,
        group,
        owner,
        0,
        &context.users[1],
        &mints[0..1],
        initial_token_deposit,
        0,
    )
    .await;
    let account_1 = create_funded_account(
        &solana,
        group,
        owner,
        1,
        &context.users[1],
        &mints[0..1],
        initial_token_deposit,
        0,
    )
    .await;

    //
    // TEST: Create a perp market
    //
    let mango_v4::accounts::PerpCreateMarket { perp_market, .. } = send_tx(
        solana,
        PerpCreateMarketInstruction {
            group,
            admin,
            payer,
            perp_market_index: 0,
            quote_lot_size: 10,
            base_lot_size: 100,
            maint_asset_weight: 0.975,
            init_asset_weight: 0.95,
            maint_liab_weight: 1.025,
            init_liab_weight: 1.05,
            liquidation_fee: 0.012,
            maker_fee: 0.0002,
            taker_fee: 0.000,
            ..PerpCreateMarketInstruction::with_new_book_and_queue(&solana, &tokens[1]).await
        },
    )
    .await
    .unwrap();

    //
    // TEST: Create another perp market
    //
    let mango_v4::accounts::PerpCreateMarket {
        perp_market: perp_market_2,
        ..
    } = send_tx(
        solana,
        PerpCreateMarketInstruction {
            group,
            admin,
            payer,
            perp_market_index: 1,
            quote_lot_size: 10,
            base_lot_size: 100,
            maint_asset_weight: 0.975,
            init_asset_weight: 0.95,
            maint_liab_weight: 1.025,
            init_liab_weight: 1.05,
            liquidation_fee: 0.012,
            maker_fee: 0.0002,
            taker_fee: 0.000,
            ..PerpCreateMarketInstruction::with_new_book_and_queue(&solana, &tokens[2]).await
        },
    )
    .await
    .unwrap();

    let price_lots = {
        let perp_market = solana.get_account::<PerpMarket>(perp_market).await;
        perp_market.native_price_to_lot(I80F48::from(1000))
    };

    // Set the initial oracle price
    send_tx(
        solana,
        StubOracleSetInstruction {
            group,
            admin,
            mint: mints[1].pubkey,
            payer,
            price: "1000.0",
        },
    )
    .await
    .unwrap();

    //
    // Place orders and create a position
    //
    send_tx(
        solana,
        PerpPlaceOrderInstruction {
            account: account_0,
            perp_market,
            owner,
            side: Side::Bid,
            price_lots,
            max_base_lots: 1,
            max_quote_lots: i64::MAX,
            client_order_id: 0,
        },
    )
    .await
    .unwrap();

    send_tx(
        solana,
        PerpPlaceOrderInstruction {
            account: account_1,
            perp_market,
            owner,
            side: Side::Ask,
            price_lots,
            max_base_lots: 1,
            max_quote_lots: i64::MAX,
            client_order_id: 0,
        },
    )
    .await
    .unwrap();

    send_tx(
        solana,
        PerpConsumeEventsInstruction {
            perp_market,
            mango_accounts: vec![account_0, account_1],
        },
    )
    .await
    .unwrap();

    {
        let mango_account_0 = solana.get_account::<MangoAccount>(account_0).await;
        let mango_account_1 = solana.get_account::<MangoAccount>(account_1).await;

        assert_eq!(mango_account_0.perps[0].base_position_lots(), 1);
        assert_eq!(mango_account_1.perps[0].base_position_lots(), -1);
        assert_eq!(
            mango_account_0.perps[0].quote_position_native().round(),
            -100_020
        );
        assert_eq!(mango_account_1.perps[0].quote_position_native(), 100_000);
    }

    // Bank must be valid for quote currency
    let result = send_tx(
        solana,
        PerpSettlePnlInstruction {
            settler,
            settler_owner,
            account_a: account_1,
            account_b: account_0,
            perp_market,
            settle_bank: tokens[1].bank,
        },
    )
    .await;

    assert_mango_error(
        &result,
        MangoError::InvalidBank.into(),
        "Bank must be valid for quote currency".to_string(),
    );

    // Cannot settle with yourself
    let result = send_tx(
        solana,
        PerpSettlePnlInstruction {
            settler,
            settler_owner,
            account_a: account_0,
            account_b: account_0,
            perp_market,
            settle_bank: tokens[0].bank,
        },
    )
    .await;

    assert_mango_error(
        &result,
        MangoError::CannotSettleWithSelf.into(),
        "Cannot settle with yourself".to_string(),
    );

    // Cannot settle position that does not exist
    let result = send_tx(
        solana,
        PerpSettlePnlInstruction {
            settler,
            settler_owner,
            account_a: account_0,
            account_b: account_1,
            perp_market: perp_market_2,
            settle_bank: tokens[0].bank,
        },
    )
    .await;

    assert_mango_error(
        &result,
        MangoError::PerpPositionDoesNotExist.into(),
        "Cannot settle a position that does not exist".to_string(),
    );

    // TODO: Test funding settlement

    {
        let mango_account_0 = solana.get_account::<MangoAccount>(account_0).await;
        let mango_account_1 = solana.get_account::<MangoAccount>(account_1).await;
        let bank = solana.get_account::<Bank>(tokens[0].bank).await;
        assert_eq!(
            mango_account_0.tokens[0].native(&bank).round(),
            initial_token_deposit,
            "account 0 has expected amount of tokens"
        );
        assert_eq!(
            mango_account_1.tokens[0].native(&bank).round(),
            initial_token_deposit,
            "account 1 has expected amount of tokens"
        );
    }

    // Try and settle with high price
    send_tx(
        solana,
        StubOracleSetInstruction {
            group,
            admin,
            mint: mints[1].pubkey,
            payer,
            price: "1200.0",
        },
    )
    .await
    .unwrap();

    // Account a must be the profitable one
    let result = send_tx(
        solana,
        PerpSettlePnlInstruction {
            settler,
            settler_owner,
            account_a: account_1,
            account_b: account_0,
            perp_market,
            settle_bank: tokens[0].bank,
        },
    )
    .await;

    assert_mango_error(
        &result,
        MangoError::ProfitabilityMismatch.into(),
        "Account a must be the profitable one".to_string(),
    );

    // Change the oracle to a more reasonable price
    send_tx(
        solana,
        StubOracleSetInstruction {
            group,
            admin,
            mint: mints[1].pubkey,
            payer,
            price: "1005.0",
        },
    )
    .await
    .unwrap();

    let expected_pnl_0 = I80F48::from(480); // Less due to fees
    let expected_pnl_1 = I80F48::from(-500);

    {
        let mango_account_0 = solana.get_account::<MangoAccount>(account_0).await;
        let mango_account_1 = solana.get_account::<MangoAccount>(account_1).await;
        let perp_market = solana.get_account::<PerpMarket>(perp_market).await;
        assert_eq!(
            get_pnl_native(&mango_account_0.perps[0], &perp_market, I80F48::from(1005)).round(),
            expected_pnl_0
        );
        assert_eq!(
            get_pnl_native(&mango_account_1.perps[0], &perp_market, I80F48::from(1005)),
            expected_pnl_1
        );
    }

    // Change the oracle to a very high price, such that the pnl exceeds the account funding
    send_tx(
        solana,
        StubOracleSetInstruction {
            group,
            admin,
            mint: mints[1].pubkey,
            payer,
            price: "1500.0",
        },
    )
    .await
    .unwrap();

    let expected_pnl_0 = I80F48::from(50000 - 20);
    let expected_pnl_1 = I80F48::from(-50000);

    {
        let mango_account_0 = solana.get_account::<MangoAccount>(account_0).await;
        let mango_account_1 = solana.get_account::<MangoAccount>(account_1).await;
        let perp_market = solana.get_account::<PerpMarket>(perp_market).await;
        assert_eq!(
            get_pnl_native(&mango_account_0.perps[0], &perp_market, I80F48::from(1500)).round(),
            expected_pnl_0
        );
        assert_eq!(
            get_pnl_native(&mango_account_1.perps[0], &perp_market, I80F48::from(1500)),
            expected_pnl_1
        );
    }

    // Settle as much PNL as account_1's health allows
    let account_1_health_non_perp = I80F48::from_num(0.8 * 10000.0);
    let expected_total_settle = account_1_health_non_perp;
    send_tx(
        solana,
        PerpSettlePnlInstruction {
            settler,
            settler_owner,
            account_a: account_0,
            account_b: account_1,
            perp_market,
            settle_bank: tokens[0].bank,
        },
    )
    .await
    .unwrap();

    {
        let bank = solana.get_account::<Bank>(tokens[0].bank).await;
        let mango_account_0 = solana.get_account::<MangoAccount>(account_0).await;
        let mango_account_1 = solana.get_account::<MangoAccount>(account_1).await;

        assert_eq!(
            mango_account_0.perps[0].base_position_lots(),
            1,
            "base position unchanged for account 0"
        );
        assert_eq!(
            mango_account_1.perps[0].base_position_lots(),
            -1,
            "base position unchanged for account 1"
        );

        assert_eq!(
            mango_account_0.perps[0].quote_position_native().round(),
            I80F48::from(-100_020) - expected_total_settle,
            "quote position reduced for profitable position"
        );
        assert_eq!(
            mango_account_1.perps[0].quote_position_native().round(),
            I80F48::from(100_000) + expected_total_settle,
            "quote position increased for losing position by opposite of first account"
        );

        assert_eq!(
            mango_account_0.tokens[0].native(&bank).round(),
            I80F48::from(initial_token_deposit) + expected_total_settle,
            "account 0 token native position increased (profit)"
        );
        assert_eq!(
            mango_account_1.tokens[0].native(&bank).round(),
            I80F48::from(initial_token_deposit) - expected_total_settle,
            "account 1 token native position decreased (loss)"
        );

        assert_eq!(
            mango_account_0.perp_spot_transfers, expected_total_settle,
            "net_settled on account 0 updated with profit from settlement"
        );
        assert_eq!(
            mango_account_1.perp_spot_transfers, -expected_total_settle,
            "net_settled on account 1 updated with loss from settlement"
        );
    }

    // Change the oracle to a reasonable price in other direction
    send_tx(
        solana,
        StubOracleSetInstruction {
            group,
            admin,
            mint: mints[1].pubkey,
            payer,
            price: "995.0",
        },
    )
    .await
    .unwrap();

    let expected_pnl_0 = I80F48::from(-8520);
    let expected_pnl_1 = I80F48::from(8500);

    {
        let mango_account_0 = solana.get_account::<MangoAccount>(account_0).await;
        let mango_account_1 = solana.get_account::<MangoAccount>(account_1).await;
        let perp_market = solana.get_account::<PerpMarket>(perp_market).await;
        assert_eq!(
            get_pnl_native(&mango_account_0.perps[0], &perp_market, I80F48::from(995)).round(),
            expected_pnl_0
        );
        assert_eq!(
            get_pnl_native(&mango_account_1.perps[0], &perp_market, I80F48::from(995)).round(),
            expected_pnl_1
        );
    }

    // Fully execute the settle
    let expected_total_settle = expected_total_settle - expected_pnl_1;
    send_tx(
        solana,
        PerpSettlePnlInstruction {
            settler,
            settler_owner,
            account_a: account_1,
            account_b: account_0,
            perp_market,
            settle_bank: tokens[0].bank,
        },
    )
    .await
    .unwrap();

    {
        let bank = solana.get_account::<Bank>(tokens[0].bank).await;
        let mango_account_0 = solana.get_account::<MangoAccount>(account_0).await;
        let mango_account_1 = solana.get_account::<MangoAccount>(account_1).await;

        assert_eq!(
            mango_account_0.perps[0].base_position_lots(),
            1,
            "base position unchanged for account 0"
        );
        assert_eq!(
            mango_account_1.perps[0].base_position_lots(),
            -1,
            "base position unchanged for account 1"
        );

        assert_eq!(
            mango_account_0.perps[0].quote_position_native().round(),
            I80F48::from(-100_020) - expected_total_settle,
            "quote position increased for losing position"
        );
        assert_eq!(
            mango_account_1.perps[0].quote_position_native().round(),
            I80F48::from(100_000) + expected_total_settle,
            "quote position reduced for losing position by opposite of first account"
        );

        // 480 was previous settlement
        assert_eq!(
            mango_account_0.tokens[0].native(&bank).round(),
            I80F48::from(initial_token_deposit) + expected_total_settle,
            "account 0 token native position decreased (loss)"
        );
        assert_eq!(
            mango_account_1.tokens[0].native(&bank).round(),
            I80F48::from(initial_token_deposit) - expected_total_settle,
            "account 1 token native position increased (profit)"
        );

        assert_eq!(
            mango_account_0.perp_spot_transfers, expected_total_settle,
            "net_settled on account 0 updated with loss from settlement"
        );
        assert_eq!(
            mango_account_1.perp_spot_transfers, -expected_total_settle,
            "net_settled on account 1 updated with profit from settlement"
        );
    }

    // no more settleable pnl left
    {
        let mango_account_0 = solana.get_account::<MangoAccount>(account_0).await;
        let mango_account_1 = solana.get_account::<MangoAccount>(account_1).await;
        let perp_market = solana.get_account::<PerpMarket>(perp_market).await;
        assert_eq!(
            get_pnl_native(&mango_account_0.perps[0], &perp_market, I80F48::from(995)).round(),
            -20 // fees
        );
        assert_eq!(
            get_pnl_native(&mango_account_1.perps[0], &perp_market, I80F48::from(995)).round(),
            0
        );
    }

    Ok(())
}

#[tokio::test]
async fn test_perp_settle_pnl_fees() -> Result<(), TransportError> {
    let context = TestContext::new().await;
    let solana = &context.solana.clone();

    let admin = TestKeypair::new();
    let owner = context.users[0].key;
    let payer = context.users[1].key;
    let mints = &context.mints[0..=2];

    let initial_token_deposit = 10_000;

    //
    // SETUP: Create a group and accounts
    //

    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        zero_token_is_quote: true,
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;
    let settle_bank = tokens[0].bank;

    // ensure vaults are not empty
    create_funded_account(
        &solana,
        group,
        owner,
        250,
        &context.users[1],
        mints,
        100_000,
        0,
    )
    .await;

    let settler =
        create_funded_account(&solana, group, owner, 251, &context.users[1], &[], 0, 0).await;
    let settler_owner = owner.clone();

    let account_0 = create_funded_account(
        &solana,
        group,
        owner,
        0,
        &context.users[1],
        &mints[0..1],
        initial_token_deposit,
        0,
    )
    .await;
    let account_1 = create_funded_account(
        &solana,
        group,
        owner,
        1,
        &context.users[1],
        &mints[0..1],
        initial_token_deposit,
        0,
    )
    .await;

    //
    // SETUP: Create a perp market
    //
    let flat_fee = 1000;
    let fee_low_health = 0.05;
    let mango_v4::accounts::PerpCreateMarket { perp_market, .. } = send_tx(
        solana,
        PerpCreateMarketInstruction {
            group,
            admin,
            payer,
            perp_market_index: 0,
            quote_lot_size: 10,
            base_lot_size: 100,
            maint_asset_weight: 1.0,
            init_asset_weight: 1.0,
            maint_liab_weight: 1.0,
            init_liab_weight: 1.0,
            liquidation_fee: 0.0,
            maker_fee: 0.0,
            taker_fee: 0.0,
            settle_fee_flat: flat_fee as f32,
            settle_fee_amount_threshold: 2000.0,
            settle_fee_fraction_low_health: fee_low_health,
            ..PerpCreateMarketInstruction::with_new_book_and_queue(&solana, &tokens[1]).await
        },
    )
    .await
    .unwrap();

    let price_lots = {
        let perp_market = solana.get_account::<PerpMarket>(perp_market).await;
        perp_market.native_price_to_lot(I80F48::from(1000))
    };

    // Set the initial oracle price
    send_tx(
        solana,
        StubOracleSetInstruction {
            group,
            admin,
            mint: mints[1].pubkey,
            payer,
            price: "1000.0",
        },
    )
    .await
    .unwrap();

    //
    // SETUP: Create a perp base position
    //
    send_tx(
        solana,
        PerpPlaceOrderInstruction {
            account: account_0,
            perp_market,
            owner,
            side: Side::Bid,
            price_lots,
            max_base_lots: 1,
            max_quote_lots: i64::MAX,
            client_order_id: 0,
        },
    )
    .await
    .unwrap();

    send_tx(
        solana,
        PerpPlaceOrderInstruction {
            account: account_1,
            perp_market,
            owner,
            side: Side::Ask,
            price_lots,
            max_base_lots: 1,
            max_quote_lots: i64::MAX,
            client_order_id: 0,
        },
    )
    .await
    .unwrap();

    send_tx(
        solana,
        PerpConsumeEventsInstruction {
            perp_market,
            mango_accounts: vec![account_0, account_1],
        },
    )
    .await
    .unwrap();

    {
        let mango_account_0 = solana.get_account::<MangoAccount>(account_0).await;
        let mango_account_1 = solana.get_account::<MangoAccount>(account_1).await;

        assert_eq!(mango_account_0.perps[0].base_position_lots(), 1);
        assert_eq!(mango_account_1.perps[0].base_position_lots(), -1);
        assert_eq!(
            mango_account_0.perps[0].quote_position_native().round(),
            -100_000
        );
        assert_eq!(mango_account_1.perps[0].quote_position_native(), 100_000);
    }

    //
    // TEST: Settle (health is high)
    //
    send_tx(
        solana,
        StubOracleSetInstruction {
            group,
            admin,
            mint: mints[1].pubkey,
            payer,
            price: "1050.0",
        },
    )
    .await
    .unwrap();

    let expected_pnl = 5000;

    send_tx(
        solana,
        PerpSettlePnlInstruction {
            settler,
            settler_owner,
            account_a: account_0,
            account_b: account_1,
            perp_market,
            settle_bank,
        },
    )
    .await
    .unwrap();

    let mut total_settled_pnl = expected_pnl;
    let mut total_fees_paid = flat_fee;
    {
        let mango_account_0 = solana.get_account::<MangoAccount>(account_0).await;
        let mango_account_1 = solana.get_account::<MangoAccount>(account_1).await;
        assert_eq!(
            mango_account_0.perps[0].quote_position_native().round(),
            I80F48::from(-100_000 - total_settled_pnl)
        );
        assert_eq!(
            mango_account_1.perps[0].quote_position_native().round(),
            I80F48::from(100_000 + total_settled_pnl),
        );
        assert_eq!(
            account_position(solana, account_0, settle_bank).await,
            initial_token_deposit as i64 + total_settled_pnl - total_fees_paid
        );
        assert_eq!(
            account_position(solana, account_1, settle_bank).await,
            initial_token_deposit as i64 - total_settled_pnl
        );
        assert_eq!(
            account_position(solana, settler, settle_bank).await,
            total_fees_paid
        );
    }

    //
    // Bring account_0 health low, specifically to
    // init_health = 14000 - 1.4 * 1 * 10700 = -980
    // maint_health = 14000 - 1.2 * 1 * 10700 = 1160
    //
    send_tx(
        solana,
        TokenWithdrawInstruction {
            account: account_0,
            owner,
            token_account: context.users[1].token_accounts[2],
            amount: 1,
            allow_borrow: true,
            bank_index: 0,
        },
    )
    .await
    .unwrap();
    send_tx(
        solana,
        StubOracleSetInstruction {
            group,
            admin,
            mint: mints[2].pubkey,
            payer,
            price: "10700.0",
        },
    )
    .await
    .unwrap();

    //
    // TEST: Settle (health is low)
    //
    send_tx(
        solana,
        StubOracleSetInstruction {
            group,
            admin,
            mint: mints[1].pubkey,
            payer,
            price: "1100.0",
        },
    )
    .await
    .unwrap();

    let expected_pnl = 5000;

    send_tx(
        solana,
        PerpSettlePnlInstruction {
            settler,
            settler_owner,
            account_a: account_0,
            account_b: account_1,
            perp_market,
            settle_bank,
        },
    )
    .await
    .unwrap();

    total_settled_pnl += expected_pnl;
    total_fees_paid += flat_fee
        + (expected_pnl as f64 * fee_low_health as f64 * 980.0 / (1160.0 + 980.0)) as i64
        + 1;
    {
        let mango_account_0 = solana.get_account::<MangoAccount>(account_0).await;
        let mango_account_1 = solana.get_account::<MangoAccount>(account_1).await;
        assert_eq!(
            mango_account_0.perps[0].quote_position_native().round(),
            I80F48::from(-100_000 - total_settled_pnl)
        );
        assert_eq!(
            mango_account_1.perps[0].quote_position_native().round(),
            I80F48::from(100_000 + total_settled_pnl),
        );
        assert_eq!(
            account_position(solana, account_0, settle_bank).await,
            initial_token_deposit as i64 + total_settled_pnl - total_fees_paid
        );
        assert_eq!(
            account_position(solana, account_1, settle_bank).await,
            initial_token_deposit as i64 - total_settled_pnl
        );
        assert_eq!(
            account_position(solana, settler, settle_bank).await,
            total_fees_paid
        );
    }

    Ok(())
}