use price_oracle::*;
use radex::radex::*;
use scrypto::prelude::*;

const TOKEN_START_PRICE: Decimal = Decimal(10); // [USD Stablecoin]
const MAX_PERFORMANCE_FEE: Decimal = Decimal(20); // [%]

blueprint! {
    struct InvestmentPool {
        /// HashMap to store all Vaults of the resources held by this pool.
        pool_vaults: HashMap<ResourceAddress, Vault>,
        /// Address of the pool token that represents ownership shares of the pools assets.
        pool_token_address: ResourceAddress,
        /// Badge that provides authorization to the investment_pool component to mint pool tokens
        pool_token_mint_badge: Vault,
        /// Badge that represents the rights of the fund manager
        pool_manager_badge_address: ResourceAddress,
        /// Decimal value, that holds the performance fee.
        performance_fee: Decimal,
        /// Vault for minted tokens due to performance fees. Can only be emptied by the fond manager.
        performance_fee_vault: Vault,
        // Holds the value of the previous high water mark of the pool token token price.
        high_water_mark: Decimal,
        /// Stores the address of the price-oracle used for this InvestmentPool
        oracle: PriceOracle,
        /// Stores the address of the decentralized exchange (DEX) thats being used for this pool.
        dex: RaDEX,
        /// Address of the base currency used in this pool.
        base_currency: ResourceAddress,
    }

    /// # This is an implementation of a Investment Pool.
    /// TODO Insert good documentation
    impl InvestmentPool {
        pub fn instantiate_pool(
            performance_fee: Decimal,
            oracle_address: ComponentAddress,
            dex_address: ComponentAddress,
            base_currency: ResourceAddress,
            fund_name: String,
            fund_symbol: String,
        ) -> (ComponentAddress, Bucket) {
            // Performing the checks to see if this liquidity pool may be created or not.
            // assert_ne!(
            //     token1.resource_address(), token2.resource_address(),
            //     "[Pool Creation]: Liquidity pools may only be created between two different tokens."
            // );

            // TODO Implement all necessary assertions.
            // Assert length of "symbol"
            // Assert that the given oracle address is really the oracle we'd expect.

            // At this point, we know that the pool creation can indeed go through.

            info!(
                "[Pool Creation]: Creating a new investment pool with the name: {} and symbol: {}.",
                fund_name, fund_symbol
            );

            // Create the Hash Map. This will later be used to store all Vaults with the pools assets.
            // let mut pool_vaults: HashMap<ResourceAddress, Vault> = HashMap::new();

            // Creating the admin badge of the investment pool which will be given the authority to mint and burn the
            // pools tracking tokens.
            let pool_token_mint_badge: Bucket = ResourceBuilder::new_fungible()
                .divisibility(DIVISIBILITY_NONE)
                .metadata("name", "Pool Token Admin Badge")
                .metadata("symbol", "PTAB")
                .metadata(
                    "description",
                    "This is an admin badge that has the authority to mint and burn pool tokens",
                )
                .initial_supply(1);

            // Creating the tracking tokens and minting the amount owed to the initial liquidity provider
            let pool_token_address: ResourceAddress = ResourceBuilder::new_fungible()
                    .divisibility(DIVISIBILITY_MAXIMUM)
                    .metadata("name", fund_name)
                    .metadata("symbol", "TT")
                    .metadata("description", "A tracking token used to track the percentage ownership of liquidity providers over the liquidity pool")
                    .mintable(rule!(require(pool_token_mint_badge.resource_address())), LOCKED)
                    .burnable(rule!(require(pool_token_mint_badge.resource_address())), LOCKED)
                    .no_initial_supply();

            // Creating the fond manager badge of the investment pool which will be given various rights only the fund manager has.
            let pool_manager_badge: Bucket = ResourceBuilder::new_fungible()
                    .divisibility(DIVISIBILITY_NONE)
                    .metadata("name", "Pool Manager Admin Badge")
                    .metadata("symbol", "PMAB")
                    .metadata("description", "This is the badge that gives certain rights to the fund manager such as withdrawing accrued performance fees")
                    .initial_supply(Decimal::ONE);

            let access_rules: AccessRules = AccessRules::new()
                .method(
                    "fund_pool",
                    rule!(require(pool_manager_badge.resource_address())),
                )
                .method(
                    "collect_fee",
                    rule!(require(pool_manager_badge.resource_address())),
                )
                .method(
                    "trade_assets",
                    rule!(require(pool_manager_badge.resource_address())),
                )
                .method(
                    "change_fee",
                    rule!(require(pool_manager_badge.resource_address())),
                )
                .default(rule!(allow_all));

            // Creating the liquidity pool component and instantiating it
            let investment_pool: ComponentAddress = Self {
                pool_vaults: HashMap::new(),
                pool_token_address,
                pool_token_mint_badge: Vault::with_bucket(pool_token_mint_badge),
                pool_manager_badge_address: pool_manager_badge.resource_address(),
                performance_fee,
                performance_fee_vault: Vault::new(pool_token_address),
                high_water_mark: Decimal::ZERO,
                oracle: oracle_address.into(),
                dex: dex_address.into(),
                base_currency,
            }
            .instantiate()
            .add_access_check(access_rules)
            .globalize();

            (investment_pool, pool_manager_badge)
        }

        /// This method can be used to fund the pool with an arbitray asset without rebalancing.
        ///
        /// This method is only callable by the pool manager.
        /// The intention of this method is mainly to initially fund the pool with arbitrary asset without triggering
        /// an automatic rebalancing via a DEX.
        ///
        /// This method fulfills three main tasks:
        /// 1. Determine the value added by the provided funds and mint the corresponding amount of pool tokens
        /// 2. Add the provided assets to the investment pool
        /// 3. Return the newly minted pool token
        pub fn fund_pool(&mut self, funds: Bucket) -> Bucket {
            assert!(
                !funds.is_empty(),
                "Can not fund this pool from an empty bucket"
            );

            // Determine the tokens to be minted.
            let tokens_to_mint: Decimal = self.calculate_pool_tokens_to_mint(&funds, self.market_cap());

            // Add funds to the pool.
            self.deposit_to_pool(funds);

            // Add this value as a datapoint to the amount of tokens that the pool manager holds (too compute stake of the pool manager)
            // TODO

            // Return a bucket with the newly minted pool tokens.
            self.mint_pool_tokens(tokens_to_mint)
        }

        /// Collect all accrued performance fees. Only callable by the pool manager.
        ///
        /// Performance fees are determined based on the principle of a high water mark.
        /// In this method it is checked whether any performance fees have accrued.
        /// If there are some they are withdrawn.
        pub fn collect_fee(&mut self) -> Bucket {

            // This is an event to mint all newly accrued performance fees.
            let (present_token_price, _) = self.pool_token_price();
            self.eval_accrued_fees(present_token_price);

            // Take all accrued performance fees and give them to pool_manager.
            self.performance_fee_vault.take_all()
        }

        /// Perform asset swap - this is the main access point for the pool manager for trading pool assets
        ///
        /// This method is only callable by the pool manager.
        /// TODO Add argument description
        pub fn trade_assets(
            &mut self,
            asset_to_sell: ResourceAddress,
            amount_to_sell: Decimal,
            asset_to_buy: ResourceAddress
        ) {
            // All necessary assertions are made within the swap method.

            // Check whether the asset_to_sell exists in the pool and whether the quantity is sufficient.
            // TODO assert whether the asset_to_sell and assets_to_buy actually exist.
            assert!((amount_to_sell > Decimal::ZERO), "The given amount_to_sell is <= zero. This transaction can't be processed.");
            assert!(self.pool_vaults.contains_key(&asset_to_sell), "The asset_to_sell doesn't exist in this pool.");
            assert!((self.pool_vaults[&asset_to_sell].amount() >= amount_to_sell), "The asset_to_sell doesn't exist in a sufficient quantity.");
            // Check whether there is a liquidity pool on the dex for our token pair.
            assert!(self.dex.pool_exists(asset_to_sell, asset_to_buy), "No liquidity pool exists for the given address pair.");

            // Take the asset_to_sell out of its vault and swap it on the DEX against the asset_to_buy
            // unwrap() can be used here, because it was already checked whether the key exists in the hashmap.
            let vault: &mut Vault = self.pool_vaults.get_mut(&asset_to_sell).unwrap();
            let bucket_to_sell: Bucket = vault.take(amount_to_sell);

            // Swap all assets.
            let bucket_to_deposit: Bucket = self.swap_assets(bucket_to_sell, asset_to_buy);

            // Deposit the swapped asset to the pool.
            self.deposit_to_pool(bucket_to_deposit);
        }

        //// Changes the performance fee.
        ///
        /// At this version of the investment_pool the performance fee is changed instantly.
        /// In the future this should only be done with an approximate period of 42 days.
        ///
        /// # Arguments:
        ///
        /// * `performance_fee` (Decimal) - \[%] New performance fee in percent.
        pub fn change_fee(&mut self, performance_fee: Decimal) {

            assert!(performance_fee >= Decimal::ZERO, "The given fee is < 0");
            assert!(performance_fee <= MAX_PERFORMANCE_FEE, "The given fee is higher than the maximum allowed percentage.");

            // This is an event to mint all newly accrued performance fees.
            let (present_token_price, _) = self.pool_token_price();
            self.eval_accrued_fees(present_token_price);

            // Take all accrued performance fees and give them to pool_manager.
            self.performance_fee = performance_fee;

        }

        /// Main interface method to invest into this investment_pool.
        ///
        /// Investments can only be done in the initially specified base currency (for now).
        /// Indirect costs of slippage are accounted to the investor, not the pool.
        /// This means that the amount of pool_tracking tokens that get returned is computed
        /// AFTER the swap on the DEX was done.
        pub fn invest(&mut self, mut asset_to_invest: Bucket)  -> Bucket {
            // TODO Implement a value checker and a general "minimum investment" --> Reduce inefficiencies compared to gas costs.
            // TODO Or directly implement a certain "value percentage" that needs to be crossed before the assets actually get traded e.g. 1%

            assert_eq!(asset_to_invest.resource_address(), self.base_currency, "Only investments in the base currency are allowed."); // TODO Add Symbol of base currency in error message.
            assert!((asset_to_invest.amount() > Decimal::ZERO), "Bucket is empty.");

            let (present_token_price, market_cap) = self.pool_token_price();

            // Save the capital that shall be invested during this method call.
            let amount_asset_to_invest: Decimal = asset_to_invest.amount();

            // Bucket that will get filled with pool tracking tokens during this method call.
            let mut pool_token_bucket: Bucket = Bucket::new(self.pool_token_address);

            // Vec that will hold the percentage value of each asset in the pool based on total value.
            let mut percentages_vec: Vec<(ResourceAddress, Decimal)> = Vec::new();

            // Perform the whole investment process via swapping the provided base currency to the target assets in the right percentage.
            // TODO also handle the case that there are no investments yet --> Just deposit funds.
            for (asset_address, asset_vault) in self.pool_vaults.iter() {

                // Determine the percentage of total value that each asset in the pool holds.
                let value_percentage: Decimal = (asset_vault.amount() * self.get_asset_price(*asset_address)) / market_cap;
                percentages_vec.push((*asset_address, value_percentage));
            }

            // The two for loops needed to be separated as otherwise rusts ownership concept couldn't be satisfied
            // in a straight forward way (cause the iterator over self.pool_vaults would either be mutable or not
            // --> method calls using (im)mutable self would'nt work.
            for (asset_address, value_percentage) in percentages_vec.iter(){
                let partial_assets: Decimal = *value_percentage * amount_asset_to_invest;

                // Make sure that no swaps happen for assets that are the base currency (no swaps necessary).
                let swapped_asset: Bucket = if *asset_address == self.base_currency {
                    asset_to_invest.take(partial_assets)
                } else {
                    // Take the partial investment and swap it.
                    self.swap_assets(asset_to_invest.take(partial_assets), *asset_address)
                };

                // Determine amount of pool tokens to be minted.
                let tokens_to_mint: Decimal = self.calculate_pool_tokens_to_mint(&swapped_asset, market_cap);

                // Mint pool tokens and add to pool token bucket.
                pool_token_bucket.put(self.mint_pool_tokens(tokens_to_mint));

                // Add all assets to their vault in the investment_pool.
                self.deposit_to_pool(swapped_asset);
            }

            // As this is a new investment this is an event to mint previously accrued performance fees.
            self.eval_accrued_fees(present_token_price);

            // Return the original bucket in case there is anything left (e.g. due to rounding errors).
            pool_token_bucket
        }

        /// Main interface method to withdraw investments
        ///
        /// Withdrawing is always done in the initially specified base currency (for now).
        pub fn divest(&mut self, pool_tokens: Bucket) -> Bucket {

            assert_eq!(pool_tokens.resource_address(), self.pool_token_address, "Please provide tracking tokens of this pool for divestments.");
            assert!((pool_tokens.amount() > Decimal::ZERO), "Bucket is empty.");

            // Determine percentage of tokens compared to total supply.
            let total_returned_pool_tokes: Decimal = pool_tokens.amount();
            let share_of_supply: Decimal = total_returned_pool_tokes / self.pool_token_supply();

            // As funds are withdrawn this is an event to mint previously accrued performance fees.
            let (present_token_price, _) = self.pool_token_price();
            self.eval_accrued_fees(present_token_price);

            // Vec that will hold the amount of tokens for each asset in the pool that need to be swapped.
            let mut amounts_to_swap_vec: Vec<(ResourceAddress, Decimal)> = Vec::new();

            // Bucket that will hold the returned investment in the base currency.
            let mut investment_to_return: Bucket = Bucket::new(self.base_currency);

            for (asset_address, asset_vault) in self.pool_vaults.iter() {
                // Save percentage of each asset and swap it back into the base currency.
                let amount_to_swap = asset_vault.amount() * share_of_supply;
                amounts_to_swap_vec.push((*asset_address, amount_to_swap));

            }

            // The two for loops needed to be separated as otherwise rusts ownership concept couldn't be satisfied
            // in a straight forward way (cause the iterator over self.pool_vaults would either be mutable or not
            // --> method calls using (im)mutable self would'nt work.
            for (asset_address, amount_to_swap) in amounts_to_swap_vec.iter(){

                // Withdraw from the vault in the investment_pool.
                let assets_to_swap: Bucket = self.withdraw_from_pool(asset_address, *amount_to_swap);

                // Swap the given amount to the base currency. Check before whether it is already the base currency.
                let swapped_asset: Bucket = if *asset_address == self.base_currency {
                    assets_to_swap
                } else {
                    self.swap_assets(assets_to_swap, self.base_currency)
                };

                // Add to return_bucket.
                investment_to_return.put(swapped_asset);
            }

            // Burn all given pool tokens.
            self.burn_pool_tokens(pool_tokens);

            investment_to_return
           }

        /// Determine the price in USD of the given token via the Oracle provided during instantiation.
        /// This only works if the token price is actually known by the oracle and otherwise aborts.
        pub fn get_asset_price(&self, asset: ResourceAddress) -> Decimal {
            match self.oracle.get_price(self.base_currency, asset) {
                Some(token_price) => token_price,
                None => std::process::abort(),
            }
        }

        /// Returns the total supply of pool tokens.
        pub fn pool_token_supply(&self) -> Decimal {
            let pool_tokens_manager: &ResourceManager =
                borrow_resource_manager!(self.pool_token_address);
            pool_tokens_manager.total_supply()
        }

        /// Determines the present price of the pool token based on its net asset value (NAV) using the underlying price oracle.
        ///
        /// This method needs to do quite a lot of computing. Don't call this method unnecessarily often!
        ///
        /// # Returns
        ///
        /// `Decimal` - The price of the pool tracking token based on its NAV.
        /// `Decimal` - Total value of all assets in the investment pool.
        ///
        pub fn pool_token_price(&self) -> (Decimal, Decimal) {
            // Assert that there are existing pool tokens.
            assert!(
                (self.pool_token_supply() > Decimal::ZERO),
                "This pool hasn't been funded yet.´There are no pool tokens representing any value."
            );

            let mut total_value: Decimal = Decimal::ZERO;

            // Determine NAV --> Iterate through all assets, add up each total value TODO: We could also determine the value percentages of each asset in here.
            //  Iterate through all vaults of the investment_pool.
            for (asset_address, asset_vault) in self.pool_vaults.iter() {
                // Determine price and token-amount for each asset in the investment pool and add it up.
                total_value += self.get_asset_price(*asset_address) * asset_vault.amount();
            }

            assert!(
                total_value > Decimal::ZERO,
                "This pool hasn't been funded yet. Total asset value is ZERO."
            );

            // 3. Finally, determine token (NAV) price via dividing the total market_cap by the amount of existing tokens.
            ((total_value / self.pool_token_supply()), total_value)
        }

        /// Determines and returns the present market cap of the investment_pool based on its NAV.
        pub fn market_cap(&self) -> Decimal {
            let (_, marketcap) = self.pool_token_price();
            marketcap
        }

        /// Calculate the amount of pool tracking tokens that should be minted based on the provided value.
        /// # Arguments:
        ///
        /// * `bucket` (&Bucket) - Bucket that holds the assets for which an equal amount of pool tokens shall be minted.
        /// * `market_cap` (Decimal) - Total market cap of all invested assets of this investment_pool. This is provided as input to reduce overall computational footprint.
        ///
        /// # Returns:
        ///
        /// * `Decimal` - Amount of pool tokens that should be minted based on the provided value.
        fn calculate_pool_tokens_to_mint(&self, bucket: &Bucket, market_cap: Decimal) -> Decimal {
            // Determine marketcap of the provided asset.
            let provided_value = self.get_asset_price(bucket.resource_address()) * bucket.amount();

            // Determine the tokens to be minted.
            if self.pool_token_supply() == Decimal::ZERO {
                // This is the first time this pool is funded with anything! Mint first tokens with a base NAV (net asset value) of TOKEN_START_PRICE
                provided_value / TOKEN_START_PRICE
            } else {
                // The pool already has some funds. Determine tokens_to_mint.
                (provided_value / market_cap) * self.pool_token_supply()
            }
        }

        /// Evaluates whether there are newly accrued performance fees and puts them in their vault.
        ///
        /// The accrued performance fees are determined based on the principle of the "high-water-mark".
        /// According to this concept new performance fees only accrue if the previous all-time-high is surpassed on the upside.
        fn eval_accrued_fees(&mut self, present_token_price: Decimal) {

            // Only perform this algorithm if there is an actual token supply.
            if self.pool_token_supply().is_positive(){

                // Check if this the first time a high_water_mark is saved.
                if self.high_water_mark.is_zero() { self.high_water_mark = present_token_price }

                // Determine whether there are newly accrued fees.
                // New fees only accrue if the previous high-water-mark is topped by present price.
                if present_token_price > self.high_water_mark {

                    let diff_to_ath = present_token_price - self.high_water_mark;
                    let accrued_fees = (diff_to_ath / present_token_price) * (self.performance_fee/dec!(100)) * self.pool_token_supply();

                    // Mint the pool tokens to cover the accrued fees and put them into their vault.
                    self.performance_fee_vault.put(self.mint_pool_tokens(accrued_fees));

                    // Update the high-water-mark: Save the present token price (but accounted for the token inflation due to minting).
                    self.high_water_mark = present_token_price * (Decimal::ONE - (self.performance_fee/dec!(100)));
                }
            } else { // This investment_pool is empty --> Reset the high_water_mark.
                self.high_water_mark = Decimal::ZERO;
            }
        }

        /// Method that mints the given amount of pool tokens.
        fn mint_pool_tokens(&self, amnt_tokens_to_mint: Decimal) -> Bucket {
            assert!(
                amnt_tokens_to_mint > Decimal::ZERO,
                "The given amount of tokens to mint is <= ZERO."
            );

            let pool_tokens_manager: &ResourceManager =
                borrow_resource_manager!(self.pool_token_address);
            self.pool_token_mint_badge
                .authorize(|| pool_tokens_manager.mint(amnt_tokens_to_mint)) // Returns bucket with new pool tokens.
        }

        /// Method that burns the given amount of pool tokens.
        fn burn_pool_tokens(&self, tokens_to_burn: Bucket) {
            assert!(
                tokens_to_burn.amount() > Decimal::ZERO,
                "The amount of given tokens to burn is <= ZERO."
            );

            let pool_tokens_manager: &ResourceManager =
                borrow_resource_manager!(self.pool_token_address);
            self.pool_token_mint_badge
                .authorize(|| pool_tokens_manager.burn(tokens_to_burn)) // Returns bucket with new pool tokens.
        }

        /// Method that deposits assets to the pool.
        fn deposit_to_pool(&mut self, bucket: Bucket) {
            // Assert whether bucket is empty.
            assert!(!bucket.is_empty(), "Bucket is empty.");

            // 1. Check whether the asset is already in the pool.
            match self.pool_vaults.get_mut(&bucket.resource_address()) {
                // 2. If yes, add to the vault
                Some(asset_vault) => {
                    asset_vault.put(bucket);
                    info!("Added the given asset to the existing vault in the investment pool.");
                }
                // 3. If no, add new vault with the provided asset.
                None => {
                    self.pool_vaults
                        .insert(bucket.resource_address(), Vault::with_bucket(bucket));
                    info!("Opened a new vault in the investment pool for the given asset.");
                }
            }
        }

        /// Method that withdraws assets from the pool.
        fn withdraw_from_pool(&mut self, address: &ResourceAddress, amount: Decimal) -> Bucket{

            assert!(self.pool_vaults.contains_key(address), "The asset_to_sell doesn't exist in this pool.");
            assert!(amount.is_positive(), "Given amount is <= ZERO.");
            assert!(self.pool_vaults[address].amount() > amount, "The balance of this vault is too low for the given amount.");

            // Withdraw asset from vault and put it into the bucket.
            let vault: &mut Vault = self.pool_vaults.get_mut(address).unwrap();
            let bucket: Bucket = vault.take(amount);

            info!("Withdraw the given asset from the invest_pool.");

            if vault.is_empty() {
                self.pool_vaults.remove(address);
                info!("The vault was empty after withdrawal --> It was removed from the investment_pool.")
            }

            bucket
        }

        /// This method handles all asset swaps that are performed within the investment_pool component.
        ///
        /// Note: at the present version, there is no optimization for slippage. Assets are just swapped "blindly" on the DEX.
        fn swap_assets(
            &self,
            bucket_to_sell: Bucket,
            asset_to_buy: ResourceAddress,
        ) -> Bucket {
            assert!(bucket_to_sell.amount() > Decimal::ZERO, "Bucket is empty. Nothing to swap.");
            self.dex.swap(bucket_to_sell, asset_to_buy)
        }
    }
}
