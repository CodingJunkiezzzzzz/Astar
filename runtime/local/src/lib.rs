#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime!` does a lot of recursion and requires us to increase the limit to 256.
#![recursion_limit = "256"]

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::{
    construct_runtime, parameter_types,
    traits::{
        ConstU128, ConstU32, Currency, EitherOfDiverse, EqualPrivilegeOnly, FindAuthor, Get,
        InstanceFilter, KeyOwnerProofSystem, Nothing,
    },
    weights::{
        constants::{RocksDbWeight, WEIGHT_PER_SECOND},
        ConstantMultiplier, IdentityFee, Weight,
    },
    ConsensusEngineId, PalletId,
};
use frame_system::limits::{BlockLength, BlockWeights};
use pallet_contracts::DefaultContractAccessWeight;
use pallet_evm::{FeeCalculator, Runner};
use pallet_evm_precompile_assets_erc20::AddressToAssetId;
use pallet_grandpa::{fg_primitives, AuthorityList as GrandpaAuthorityList};
use sp_api::impl_runtime_apis;
use sp_core::{crypto::KeyTypeId, OpaqueMetadata, H160, H256, U256};
use sp_runtime::{
    create_runtime_str, generic, impl_opaque_keys,
    traits::{
        AccountIdConversion, AccountIdLookup, BlakeTwo256, Block as BlockT, ConvertInto,
        DispatchInfoOf, Dispatchable, IdentifyAccount, NumberFor, PostDispatchInfoOf,
        UniqueSaturatedInto, Verify,
    },
    transaction_validity::{
        TransactionPriority, TransactionSource, TransactionValidity, TransactionValidityError,
    },
    ApplyExtrinsicResult, MultiSignature, RuntimeDebug,
};
use sp_std::prelude::*;

#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

pub use pallet_balances::Call as BalancesCall;
pub use pallet_grandpa::AuthorityId as GrandpaId;
pub use pallet_timestamp::Call as TimestampCall;
use pallet_transaction_payment::CurrencyAdapter;
pub use sp_consensus_aura::sr25519::AuthorityId as AuraId;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;
pub use sp_runtime::{Perbill, Permill};

mod weights;

#[cfg(feature = "std")]
/// Wasm binary unwrapped. If built with `BUILD_DUMMY_WASM_BINARY`, the function panics.
pub fn wasm_binary_unwrap() -> &'static [u8] {
    WASM_BINARY.expect(
        "Development wasm binary is not available. This means the client is \
                        built with `BUILD_DUMMY_WASM_BINARY` flag and it is only usable for \
                        production chains. Please rebuild with the flag disabled.",
    )
}

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
    spec_name: create_runtime_str!("astar-local"),
    impl_name: create_runtime_str!("astar-local"),
    authoring_version: 1,
    spec_version: 1,
    impl_version: 1,
    apis: RUNTIME_API_VERSIONS,
    transaction_version: 1,
    state_version: 1,
};

impl_opaque_keys! {
    pub struct SessionKeys {
        pub aura: Aura,
        pub grandpa: Grandpa,
    }
}

mod precompiles;
pub use precompiles::{LocalNetworkPrecompiles, ASSET_PRECOMPILE_ADDRESS_PREFIX};
pub type Precompiles = LocalNetworkPrecompiles<Runtime>;

mod chain_extensions;
pub use chain_extensions::*;

/// Constant values used within the runtime.
pub const MILLIAST: Balance = 1_000_000_000_000_000;
pub const AST: Balance = 1_000 * MILLIAST;

/// Charge fee for stored bytes and items.
pub const fn deposit(items: u32, bytes: u32) -> Balance {
    (items as Balance + bytes as Balance) * MILLIAST / 1_000_000
}

/// This determines the average expected block time that we are targeting.
/// Blocks will be produced at a minimum duration defined by `SLOT_DURATION`.
/// `SLOT_DURATION` is picked up by `pallet_timestamp` which is in turn picked
/// up by `pallet_aura` to implement `fn slot_duration()`.
///
/// Change this to adjust the block time.
pub const MILLISECS_PER_BLOCK: u64 = 2000;
pub const SLOT_DURATION: u64 = MILLISECS_PER_BLOCK;

// Time is measured by number of blocks.
pub const MINUTES: BlockNumber = 60_000 / (MILLISECS_PER_BLOCK as BlockNumber);
pub const HOURS: BlockNumber = MINUTES * 60;
pub const DAYS: BlockNumber = HOURS * 24;

pub type AssetId = u128;

impl AddressToAssetId<AssetId> for Runtime {
    fn address_to_asset_id(address: H160) -> Option<AssetId> {
        let mut data = [0u8; 16];
        let address_bytes: [u8; 20] = address.into();
        if ASSET_PRECOMPILE_ADDRESS_PREFIX.eq(&address_bytes[0..4]) {
            data.copy_from_slice(&address_bytes[4..20]);
            Some(u128::from_be_bytes(data))
        } else {
            None
        }
    }

    fn asset_id_to_address(asset_id: AssetId) -> H160 {
        let mut data = [0u8; 20];
        data[0..4].copy_from_slice(ASSET_PRECOMPILE_ADDRESS_PREFIX);
        data[4..20].copy_from_slice(&asset_id.to_be_bytes());
        H160::from(data)
    }
}

/// The version information used to identify this runtime when compiled natively.
#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
    NativeVersion {
        runtime_version: VERSION,
        can_author_with: Default::default(),
    }
}

/// We allow `Normal` extrinsics to fill up the block up to 75%, the rest can be used
/// by  Operational  extrinsics.
const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);
/// We assume that ~10% of the block weight is consumed by `on_initalize` handlers.
/// This is used to limit the maximal weight of a single extrinsic.
const AVERAGE_ON_INITIALIZE_RATIO: Perbill = Perbill::from_percent(10);

parameter_types! {
    pub const Version: RuntimeVersion = VERSION;
    pub const BlockHashCount: BlockNumber = 2400;
    /// We allow for 1 seconds of compute with a 2 second average block time.
    pub RuntimeBlockWeights: BlockWeights = BlockWeights
        ::with_sensible_defaults(WEIGHT_PER_SECOND, NORMAL_DISPATCH_RATIO);
    pub RuntimeBlockLength: BlockLength = BlockLength
        ::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
    pub const SS58Prefix: u8 = 5;
}

// Configure FRAME pallets to include in runtime.

impl frame_system::Config for Runtime {
    /// The basic call filter to use in dispatchable.
    type BaseCallFilter = frame_support::traits::Everything;
    /// Block & extrinsics weights: base values and limits.
    type BlockWeights = RuntimeBlockWeights;
    /// The maximum length of a block (in bytes).
    type BlockLength = RuntimeBlockLength;
    /// The identifier used to distinguish between accounts.
    type AccountId = AccountId;
    /// The aggregated dispatch type that is available for extrinsics.
    type RuntimeCall = RuntimeCall;
    /// The lookup mechanism to get account ID from whatever is passed in dispatchers.
    type Lookup = AccountIdLookup<AccountId, ()>;
    /// The index type for storing how many extrinsics an account has signed.
    type Index = Index;
    /// The index type for blocks.
    type BlockNumber = BlockNumber;
    /// The type for hashing blocks and tries.
    type Hash = Hash;
    /// The hashing algorithm used.
    type Hashing = BlakeTwo256;
    /// The header type.
    type Header = generic::Header<BlockNumber, BlakeTwo256>;
    /// The ubiquitous event type.
    type RuntimeEvent = RuntimeEvent;
    /// The ubiquitous origin type.
    type RuntimeOrigin = RuntimeOrigin;
    /// Maximum number of block number to block hash mappings to keep (oldest pruned first).
    type BlockHashCount = BlockHashCount;
    /// The weight of database operations that the runtime can invoke.
    type DbWeight = RocksDbWeight;
    /// Version of the runtime.
    type Version = Version;
    /// Converts a module to the index of the module in `construct_runtime!`.
    ///
    /// This type is being generated by `construct_runtime!`.
    type PalletInfo = PalletInfo;
    /// What to do if a new account is created.
    type OnNewAccount = ();
    /// What to do if an account is fully reaped from the system.
    type OnKilledAccount = ();
    /// The data to be stored in an account.
    type AccountData = pallet_balances::AccountData<Balance>;
    /// Weight information for the extrinsics of this pallet.
    type SystemWeightInfo = ();
    /// This is used as an identifier of the chain. 42 is the generic substrate prefix.
    type SS58Prefix = SS58Prefix;
    /// The set code logic, just the default since we're not a parachain.
    type OnSetCode = ();
    type MaxConsumers = frame_support::traits::ConstU32<16>;
}

parameter_types! {
    pub const MaxAuthorities: u32 = 50;
}

impl pallet_aura::Config for Runtime {
    type AuthorityId = AuraId;
    type DisabledValidators = ();
    type MaxAuthorities = MaxAuthorities;
}

impl pallet_grandpa::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;

    type KeyOwnerProofSystem = ();

    type KeyOwnerProof =
        <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(KeyTypeId, GrandpaId)>>::Proof;

    type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
        KeyTypeId,
        GrandpaId,
    )>>::IdentificationTuple;

    type HandleEquivocation = ();

    type WeightInfo = ();
    type MaxAuthorities = MaxAuthorities;
}

parameter_types! {
    pub const MinimumPeriod: u64 = SLOT_DURATION / 2;
}

impl pallet_timestamp::Config for Runtime {
    /// A timestamp: milliseconds since the unix epoch.
    type Moment = u64;
    type OnTimestampSet = (Aura, BlockReward);
    type MinimumPeriod = MinimumPeriod;
    type WeightInfo = ();
}

impl pallet_randomness_collective_flip::Config for Runtime {}

parameter_types! {
    pub const ExistentialDeposit: u128 = 500;
    pub const MaxLocks: u32 = 50;
}

impl pallet_balances::Config for Runtime {
    type MaxLocks = MaxLocks;
    type MaxReserves = ();
    type ReserveIdentifier = [u8; 8];
    /// The type for recording an account's balance.
    type Balance = Balance;
    /// The ubiquitous event type.
    type RuntimeEvent = RuntimeEvent;
    type DustRemoval = ();
    type ExistentialDeposit = ExistentialDeposit;
    type AccountStore = System;
    type WeightInfo = pallet_balances::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
    pub const AssetDeposit: Balance = 1_000_000;
    pub const ApprovalDeposit: Balance = 1_000_000;
    pub const AssetsStringLimit: u32 = 50;
    /// Key = 32 bytes, Value = 36 bytes (32+1+1+1+1)
    // https://github.com/paritytech/substrate/blob/069917b/frame/assets/src/lib.rs#L257L271
    pub const MetadataDepositBase: Balance = deposit(1, 68);
    pub const MetadataDepositPerByte: Balance = deposit(0, 1);
    pub const AssetAccountDeposit: Balance = deposit(1, 18);
}

impl pallet_assets::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Balance = Balance;
    type AssetId = AssetId;
    type Currency = Balances;
    type ForceOrigin = frame_system::EnsureRoot<AccountId>;
    type AssetDeposit = AssetDeposit;
    type MetadataDepositBase = MetadataDepositBase;
    type MetadataDepositPerByte = MetadataDepositPerByte;
    type AssetAccountDeposit = AssetAccountDeposit;
    type ApprovalDeposit = ApprovalDeposit;
    type StringLimit = AssetsStringLimit;
    type Freezer = ();
    type Extra = ();
    type WeightInfo = pallet_assets::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
    pub const TransactionByteFee: Balance = 1;
    pub const OperationalFeeMultiplier: u8 = 5;
}

impl pallet_transaction_payment::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type OnChargeTransaction = CurrencyAdapter<Balances, ()>;
    type WeightToFee = IdentityFee<Balance>;
    type OperationalFeeMultiplier = OperationalFeeMultiplier;
    type FeeMultiplierUpdate = ();
    type LengthToFee = ConstantMultiplier<Balance, TransactionByteFee>;
}

parameter_types! {
    pub const TreasuryPalletId: PalletId = PalletId(*b"py/trsry");
    pub const DappsStakingPalletId: PalletId = PalletId(*b"py/dpsst");
}

type NegativeImbalance = <Balances as Currency<AccountId>>::NegativeImbalance;

pub struct DappsStakingTvlProvider();
impl Get<Balance> for DappsStakingTvlProvider {
    fn get() -> Balance {
        DappsStaking::tvl()
    }
}

pub struct BeneficiaryPayout();
impl pallet_block_reward::BeneficiaryPayout<NegativeImbalance> for BeneficiaryPayout {
    fn treasury(reward: NegativeImbalance) {
        Balances::resolve_creating(&TreasuryPalletId::get().into_account_truncating(), reward);
    }

    fn collators(_reward: NegativeImbalance) {
        // no collators for local dev node
    }

    fn dapps_staking(stakers: NegativeImbalance, dapps: NegativeImbalance) {
        DappsStaking::rewards(stakers, dapps)
    }
}

parameter_types! {
    pub const RewardAmount: Balance = 2_664 * MILLIAST;
}

impl pallet_block_reward::Config for Runtime {
    type Currency = Balances;
    type DappsStakingTvlProvider = DappsStakingTvlProvider;
    type BeneficiaryPayout = BeneficiaryPayout;
    type RewardAmount = RewardAmount;
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = pallet_block_reward::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
    pub const BlockPerEra: BlockNumber = 60;
    pub const RegisterDeposit: Balance = 100 * AST;
    pub const MaxNumberOfStakersPerContract: u32 = 512;
    pub const MinimumStakingAmount: Balance = 10 * AST;
    pub const MinimumRemainingAmount: Balance = AST;
    pub const MaxUnlockingChunks: u32 = 2;
    pub const UnbondingPeriod: u32 = 2;
    pub const MaxEraStakeValues: u32 = 5;
}

impl pallet_dapps_staking::Config for Runtime {
    type Currency = Balances;
    type BlockPerEra = BlockPerEra;
    type SmartContract = SmartContract<AccountId>;
    type RegisterDeposit = RegisterDeposit;
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = weights::pallet_dapps_staking::WeightInfo<Runtime>;
    type MaxNumberOfStakersPerContract = MaxNumberOfStakersPerContract;
    type MinimumStakingAmount = MinimumStakingAmount;
    type PalletId = DappsStakingPalletId;
    type MaxUnlockingChunks = MaxUnlockingChunks;
    type UnbondingPeriod = UnbondingPeriod;
    type MinimumRemainingAmount = MinimumRemainingAmount;
    type MaxEraStakeValues = MaxEraStakeValues;
}

/// Multi-VM pointer to smart contract instance.
#[derive(
    PartialEq, Eq, Copy, Clone, Encode, Decode, RuntimeDebug, MaxEncodedLen, scale_info::TypeInfo,
)]
pub enum SmartContract<AccountId> {
    /// EVM smart contract instance.
    Evm(sp_core::H160),
    /// Wasm smart contract instance.
    Wasm(AccountId),
}

impl<AccountId> Default for SmartContract<AccountId> {
    fn default() -> Self {
        SmartContract::Evm(H160::repeat_byte(0x00))
    }
}

impl<AccountId: From<[u8; 32]>> From<[u8; 32]> for SmartContract<AccountId> {
    fn from(input: [u8; 32]) -> Self {
        SmartContract::Wasm(input.into())
    }
}

impl pallet_utility::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type PalletsOrigin = OriginCaller;
    type WeightInfo = ();
}

parameter_types! {
    pub EvmId: u8 = 0x0F;
    pub WasmId: u8 = 0x1F;
}

use pallet_xvm::{evm, wasm};
impl pallet_xvm::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type VmId = u8;
    type SyncVM = (evm::EVM<EvmId, Self, ()>, wasm::WASM<WasmId, Self, ()>);
    type AsyncVM = ();
}

parameter_types! {
    // Tells `pallet_base_fee` whether to calculate a new BaseFee `on_finalize` or not.
    pub DefaultBaseFeePerGas: U256 = (MILLIAST / 1_000_000).into();
    // At the moment, we don't use dynamic fee calculation for local chain by default
    pub DefaultElasticity: Permill = Permill::zero();
}

pub struct BaseFeeThreshold;
impl pallet_base_fee::BaseFeeThreshold for BaseFeeThreshold {
    fn lower() -> Permill {
        Permill::zero()
    }
    fn ideal() -> Permill {
        Permill::from_parts(500_000)
    }
    fn upper() -> Permill {
        Permill::from_parts(1_000_000)
    }
}

impl pallet_base_fee::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Threshold = BaseFeeThreshold;
    type DefaultBaseFeePerGas = DefaultBaseFeePerGas;
    type DefaultElasticity = DefaultElasticity;
}

/// Current approximation of the gas/s consumption considering
/// EVM execution over compiled WASM (on 4.4Ghz CPU).
/// Given the 500ms Weight, from which 75% only are used for transactions,
/// the total EVM execution gas limit is: GAS_PER_SECOND * 0.500 * 0.75 ~= 15_000_000.
pub const GAS_PER_SECOND: u64 = 40_000_000;

/// Approximate ratio of the amount of Weight per Gas.
/// u64 works for approximations because Weight is a very small unit compared to gas.
pub const WEIGHT_PER_GAS: u64 = WEIGHT_PER_SECOND.saturating_div(GAS_PER_SECOND).ref_time();

pub struct FindAuthorTruncated<F>(sp_std::marker::PhantomData<F>);
impl<F: FindAuthor<u32>> FindAuthor<H160> for FindAuthorTruncated<F> {
    fn find_author<'a, I>(digests: I) -> Option<H160>
    where
        I: 'a + IntoIterator<Item = (ConsensusEngineId, &'a [u8])>,
    {
        if let Some(author_index) = F::find_author(digests) {
            let authority_id = Aura::authorities()[author_index as usize].clone();
            return Some(H160::from_slice(&authority_id.encode()[4..24]));
        }

        None
    }
}

parameter_types! {
    /// Ethereum-compatible chain_id:
    /// * Dusty:   80
    /// * Shibuya: 81
    /// * Shiden: 336
    /// * Local: 4369
    pub ChainId: u64 = 0x1111;
    /// EVM gas limit
    pub BlockGasLimit: U256 = U256::from(
        NORMAL_DISPATCH_RATIO * WEIGHT_PER_SECOND.ref_time() / WEIGHT_PER_GAS
    );
    pub PrecompilesValue: Precompiles = LocalNetworkPrecompiles::<_>::new();
    pub WeightPerGas: u64 = WEIGHT_PER_GAS;
}

impl pallet_evm::Config for Runtime {
    type FeeCalculator = BaseFee;
    type GasWeightMapping = pallet_evm::FixedGasWeightMapping<Self>;
    type WeightPerGas = WeightPerGas;
    type BlockHashMapping = pallet_ethereum::EthereumBlockHashMapping<Runtime>;
    type CallOrigin = pallet_evm::EnsureAddressRoot<AccountId>;
    type WithdrawOrigin = pallet_evm::EnsureAddressTruncated;
    type AddressMapping = pallet_evm::HashedAddressMapping<BlakeTwo256>;
    type Currency = Balances;
    type RuntimeEvent = RuntimeEvent;
    type Runner = pallet_evm::runner::stack::Runner<Self>;
    type PrecompilesType = Precompiles;
    type PrecompilesValue = PrecompilesValue;
    type ChainId = ChainId;
    type OnChargeTransaction = pallet_evm::EVMCurrencyAdapter<Balances, ()>;
    type BlockGasLimit = BlockGasLimit;
    type FindAuthor = FindAuthorTruncated<Aura>;
}

impl pallet_ethereum::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type StateRoot = pallet_ethereum::IntermediateStateRoot<Self>;
}

parameter_types! {
    pub const EcdsaUnsignedPriority: TransactionPriority = TransactionPriority::MAX / 2;
    pub const CallFee: Balance = AST / 10;
    pub const CallMagicNumber: u16 = 0xff51;
}

impl pallet_custom_signatures::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type Signature = pallet_custom_signatures::ethereum::EthereumSignature;
    type Signer = <Signature as Verify>::Signer;
    type CallMagicNumber = CallMagicNumber;
    type Currency = Balances;
    type CallFee = CallFee;
    type OnChargeTransaction = ();
    type UnsignedPriority = EcdsaUnsignedPriority;
}

parameter_types! {
    pub MaximumSchedulerWeight: Weight = NORMAL_DISPATCH_RATIO * RuntimeBlockWeights::get().max_block;
}

impl pallet_scheduler::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeOrigin = RuntimeOrigin;
    type PalletsOrigin = OriginCaller;
    type RuntimeCall = RuntimeCall;
    type MaximumWeight = MaximumSchedulerWeight;
    type ScheduleOrigin = frame_system::EnsureRoot<AccountId>;
    type MaxScheduledPerBlock = ConstU32<50>;
    type WeightInfo = pallet_scheduler::weights::SubstrateWeight<Runtime>;
    type OriginPrivilegeCmp = EqualPrivilegeOnly;
    type PreimageProvider = ();
    type NoPreimagePostponement = ();
}

parameter_types! {
    pub CouncilMotionDuration: BlockNumber = 3 * MINUTES;
}

type CouncilCollective = pallet_collective::Instance1;
impl pallet_collective::Config<CouncilCollective> for Runtime {
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeEvent = RuntimeEvent;
    type Proposal = RuntimeCall;
    type MotionDuration = CouncilMotionDuration;
    type MaxProposals = ConstU32<100>;
    type MaxMembers = ConstU32<3>;
    type DefaultVote = pallet_collective::PrimeDefaultVote;
    type WeightInfo = pallet_collective::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
    pub const TechnicalCommitteeMotionDuration: BlockNumber = 3 * MINUTES;
}

type TechnicalCommitteeCollective = pallet_collective::Instance2;
impl pallet_collective::Config<TechnicalCommitteeCollective> for Runtime {
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeEvent = RuntimeEvent;
    type Proposal = RuntimeCall;
    type MotionDuration = TechnicalCommitteeMotionDuration;
    type MaxProposals = ConstU32<100>;
    type MaxMembers = ConstU32<3>;
    type DefaultVote = pallet_collective::PrimeDefaultVote;
    type WeightInfo = pallet_collective::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
    pub const ProposalBond: Permill = Permill::from_percent(5);
    pub const ProposalBondMinimum: Balance = 100 * AST;
    pub const ProposalBondMaximum: Balance = 500 * AST;
    pub const SpendPeriod: BlockNumber = 1 * MINUTES;
    pub const Burn: Permill = Permill::from_percent(1);
}

impl pallet_treasury::Config for Runtime {
    type PalletId = TreasuryPalletId;
    type Currency = Balances;
    type ApproveOrigin = EitherOfDiverse<
        frame_system::EnsureRoot<AccountId>,
        pallet_collective::EnsureProportionAtLeast<AccountId, CouncilCollective, 2, 3>,
    >;
    type RejectOrigin = EitherOfDiverse<
        frame_system::EnsureRoot<AccountId>,
        pallet_collective::EnsureProportionMoreThan<AccountId, CouncilCollective, 1, 2>,
    >;
    type RuntimeEvent = RuntimeEvent;
    type OnSlash = Treasury;
    type ProposalBond = ProposalBond;
    type ProposalBondMinimum = ProposalBondMinimum;
    type ProposalBondMaximum = ProposalBondMaximum;
    type SpendPeriod = SpendPeriod;
    type Burn = Burn;
    type BurnDestination = ();
    type SpendFunds = ();
    type MaxApprovals = ConstU32<100>;
    type WeightInfo = pallet_treasury::weights::SubstrateWeight<Runtime>;
    type SpendOrigin = frame_support::traits::NeverEnsureOrigin<Balance>;
}

parameter_types! {
    pub LaunchPeriod: BlockNumber = 1 * MINUTES;
    pub VotingPeriod: BlockNumber = 3 * MINUTES;
    pub FastTrackVotingPeriod: BlockNumber = 1 * MINUTES;
    pub const MinimumDeposit: Balance = 7000 * AST;
    pub EnactmentPeriod: BlockNumber = 5 * MINUTES;
    pub VoteLockingPeriod: BlockNumber = 10 * MINUTES;
    pub CooloffPeriod: BlockNumber = 10 * MINUTES;
    pub const InstantAllowed: bool = true;
    pub const PreimageByteDeposit: Balance = deposit(0, 1);
}

impl pallet_democracy::Config for Runtime {
    type Proposal = RuntimeCall;
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type EnactmentPeriod = EnactmentPeriod;
    type LaunchPeriod = LaunchPeriod;
    type VotingPeriod = VotingPeriod;
    type VoteLockingPeriod = VoteLockingPeriod;
    type MinimumDeposit = MinimumDeposit;
    /// A straight majority of the council can decide what their next motion is.
    type ExternalOrigin = EitherOfDiverse<
        pallet_collective::EnsureProportionAtLeast<AccountId, CouncilCollective, 1, 2>,
        frame_system::EnsureRoot<AccountId>,
    >;
    /// A 60% super-majority can have the next scheduled referendum be a straight majority-carries vote.
    type ExternalMajorityOrigin = EitherOfDiverse<
        pallet_collective::EnsureProportionAtLeast<AccountId, CouncilCollective, 3, 5>,
        frame_system::EnsureRoot<AccountId>,
    >;
    /// A unanimous council can have the next scheduled referendum be a straight default-carries
    /// (NTB) vote.
    type ExternalDefaultOrigin = EitherOfDiverse<
        pallet_collective::EnsureProportionAtLeast<AccountId, CouncilCollective, 1, 1>,
        frame_system::EnsureRoot<AccountId>,
    >;
    /// Two thirds of the technical committee can have an `ExternalMajority/ExternalDefault` vote
    /// be tabled immediately and with a shorter voting/enactment period.
    type FastTrackOrigin = EitherOfDiverse<
        pallet_collective::EnsureProportionAtLeast<AccountId, TechnicalCommitteeCollective, 2, 3>,
        frame_system::EnsureRoot<AccountId>,
    >;
    type InstantOrigin = EitherOfDiverse<
        pallet_collective::EnsureProportionAtLeast<AccountId, TechnicalCommitteeCollective, 1, 1>,
        frame_system::EnsureRoot<AccountId>,
    >;
    type InstantAllowed = InstantAllowed;
    type FastTrackVotingPeriod = FastTrackVotingPeriod;
    // To cancel a proposal which has been passed, 2/3 of the council must agree to it.
    type CancellationOrigin = EitherOfDiverse<
        pallet_collective::EnsureProportionAtLeast<AccountId, CouncilCollective, 2, 3>,
        frame_system::EnsureRoot<AccountId>,
    >;
    // To cancel a proposal before it has been passed, the technical committee must be unanimous or
    // Root must agree.
    type CancelProposalOrigin = EitherOfDiverse<
        pallet_collective::EnsureProportionAtLeast<AccountId, TechnicalCommitteeCollective, 1, 1>,
        frame_system::EnsureRoot<AccountId>,
    >;
    type BlacklistOrigin = frame_system::EnsureRoot<AccountId>;
    // Any single technical committee member may veto a coming council proposal, however they can
    // only do it once and it lasts only for the cooloff period.
    type VetoOrigin = pallet_collective::EnsureMember<AccountId, TechnicalCommitteeCollective>;
    type CooloffPeriod = CooloffPeriod;
    // The amount of balance that must be deposited per byte of preimage stored.
    type PreimageByteDeposit = PreimageByteDeposit;
    type Slash = Treasury;
    type Scheduler = Scheduler;
    type MaxVotes = ConstU32<100>;
    type OperationalPreimageOrigin = pallet_collective::EnsureMember<AccountId, CouncilCollective>;
    type PalletsOrigin = OriginCaller;
    type WeightInfo = pallet_democracy::weights::SubstrateWeight<Runtime>;
    type MaxProposals = ConstU32<100>;
}

parameter_types! {
    pub const DepositPerItem: Balance = deposit(1, 0);
    pub const DepositPerByte: Balance = deposit(0, 1);
    pub const MaxValueSize: u32 = 16 * 1024;
    // The lazy deletion runs inside on_initialize.
    pub DeletionWeightLimit: Weight = AVERAGE_ON_INITIALIZE_RATIO *
        RuntimeBlockWeights::get().max_block;
    pub Schedule: pallet_contracts::Schedule<Runtime> = Default::default();
}

parameter_types! {
    pub const MinVestedTransfer: Balance = AST;
}

impl pallet_vesting::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type BlockNumberToBalance = ConvertInto;
    type MinVestedTransfer = MinVestedTransfer;
    type WeightInfo = ();
    // `VestingInfo` encode length is 36bytes. 28 schedules gets encoded as 1009 bytes, which is the
    // highest number of schedules that encodes less than 2^10.
    const MAX_VESTING_SCHEDULES: u32 = 28;
}

impl pallet_contracts::Config for Runtime {
    type Time = Timestamp;
    type Randomness = RandomnessCollectiveFlip;
    type Currency = Balances;
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    /// The safest default is to allow no calls at all.
    ///
    /// Runtimes should whitelist dispatchables that are allowed to be called from contracts
    /// and make sure they are stable. Dispatchables exposed to contracts are not allowed to
    /// change because that would break already deployed contracts. The `Call` structure itself
    /// is not allowed to change the indices of existing pallets, too.
    type CallFilter = Nothing;
    type DepositPerItem = DepositPerItem;
    type DepositPerByte = DepositPerByte;
    type CallStack = [pallet_contracts::Frame<Self>; 31];
    type WeightPrice = pallet_transaction_payment::Pallet<Self>;
    type WeightInfo = pallet_contracts::weights::SubstrateWeight<Self>;
    type ChainExtension = (DappsStakingExtension<Self>,);
    type DeletionQueueDepth = ConstU32<128>;
    type DeletionWeightLimit = DeletionWeightLimit;
    type Schedule = Schedule;
    type AddressGenerator = pallet_contracts::DefaultAddressGenerator;
    type ContractAccessWeight = DefaultContractAccessWeight<RuntimeBlockWeights>;
    type MaxCodeLen = ConstU32<{ 128 * 1024 }>;
    type MaxStorageKeyLen = ConstU32<128>;
}

impl pallet_sudo::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
}

/// The type used to represent the kinds of proxying allowed.
#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Encode,
    Decode,
    RuntimeDebug,
    MaxEncodedLen,
    scale_info::TypeInfo,
)]
pub enum ProxyType {
    Any,
}

impl Default for ProxyType {
    fn default() -> Self {
        Self::Any
    }
}
impl InstanceFilter<RuntimeCall> for ProxyType {
    fn filter(&self, _c: &RuntimeCall) -> bool {
        match self {
            ProxyType::Any => true,
        }
    }
}

impl pallet_proxy::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type Currency = Balances;
    type ProxyType = ProxyType;
    // One storage item; key size 32, value size 8; .
    type ProxyDepositBase = ConstU128<{ AST * 10 }>;
    // Additional storage item size of 33 bytes.
    type ProxyDepositFactor = ConstU128<{ MILLIAST * 330 }>;
    type MaxProxies = ConstU32<32>;
    type WeightInfo = pallet_proxy::weights::SubstrateWeight<Runtime>;
    type MaxPending = ConstU32<32>;
    type CallHasher = BlakeTwo256;
    // Key size 32 + 1 item
    type AnnouncementDepositBase = ConstU128<{ AST * 10 }>;
    // Acc Id + Hash + block number
    type AnnouncementDepositFactor = ConstU128<{ MILLIAST * 660 }>;
}

// TODO: remove this once https://github.com/paritytech/substrate/issues/12161 is resolved
#[rustfmt::skip]
construct_runtime!(
    pub struct Runtime
    where
        Block = Block,
        NodeBlock = generic::Block<Header, sp_runtime::OpaqueExtrinsic>,
        UncheckedExtrinsic = UncheckedExtrinsic,
    {
        System: frame_system,
        Utility: pallet_utility,
        Timestamp: pallet_timestamp,
        RandomnessCollectiveFlip: pallet_randomness_collective_flip,
        Aura: pallet_aura,
        Grandpa: pallet_grandpa,
        Balances: pallet_balances,
        Vesting: pallet_vesting,
        DappsStaking: pallet_dapps_staking,
        BlockReward: pallet_block_reward,
        TransactionPayment: pallet_transaction_payment,
        EVM: pallet_evm,
        Ethereum: pallet_ethereum,
        EthCall: pallet_custom_signatures,
        BaseFee: pallet_base_fee,
        Contracts: pallet_contracts,
        Sudo: pallet_sudo,
        Assets: pallet_assets,
        Scheduler: pallet_scheduler,
        Democracy: pallet_democracy,
        Council: pallet_collective::<Instance1>,
        TechnicalCommittee: pallet_collective::<Instance2>,
        Treasury: pallet_treasury,
        Xvm: pallet_xvm,
        Proxy: pallet_proxy,
    }
);

/// Balance of an account.
pub type Balance = u128;
/// Alias to 512-bit hash when used in the context of a transaction signature on the chain.
pub type Signature = MultiSignature;
/// Some way of identifying an account on the chain. We intentionally make it equivalent
/// to the public key of our transaction signing scheme.
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;
/// Index of a transaction in the chain.
pub type Index = u32;
/// A hash of some data used by the chain.
pub type Hash = sp_core::H256;
/// An index to a block.
pub type BlockNumber = u32;
/// The address format for describing accounts.
pub type Address = sp_runtime::MultiAddress<AccountId, ()>;
/// Block header type as expected by this runtime.
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;
/// BlockId type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;
/// The SignedExtension to the basic transaction logic.
pub type SignedExtra = (
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckTxVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
    frame_system::CheckEra<Runtime>,
    frame_system::CheckNonce<Runtime>,
    frame_system::CheckWeight<Runtime>,
    pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
);
/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic =
    fp_self_contained::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;
/// Extrinsic type that has already been checked.
pub type CheckedExtrinsic =
    fp_self_contained::CheckedExtrinsic<AccountId, RuntimeCall, SignedExtra, H160>;
/// The payload being signed in transactions.
pub type SignedPayload = generic::SignedPayload<RuntimeCall, SignedExtra>;
/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
    Runtime,
    Block,
    frame_system::ChainContext<Runtime>,
    Runtime,
    AllPalletsWithSystem,
>;

impl fp_self_contained::SelfContainedCall for RuntimeCall {
    type SignedInfo = H160;

    fn is_self_contained(&self) -> bool {
        match self {
            RuntimeCall::Ethereum(call) => call.is_self_contained(),
            _ => false,
        }
    }

    fn check_self_contained(&self) -> Option<Result<Self::SignedInfo, TransactionValidityError>> {
        match self {
            RuntimeCall::Ethereum(call) => call.check_self_contained(),
            _ => None,
        }
    }

    fn validate_self_contained(
        &self,
        info: &Self::SignedInfo,
        dispatch_info: &DispatchInfoOf<RuntimeCall>,
        len: usize,
    ) -> Option<TransactionValidity> {
        match self {
            RuntimeCall::Ethereum(call) => call.validate_self_contained(info, dispatch_info, len),
            _ => None,
        }
    }

    fn pre_dispatch_self_contained(
        &self,
        info: &Self::SignedInfo,
        dispatch_info: &DispatchInfoOf<RuntimeCall>,
        len: usize,
    ) -> Option<Result<(), TransactionValidityError>> {
        match self {
            RuntimeCall::Ethereum(call) => {
                call.pre_dispatch_self_contained(info, dispatch_info, len)
            }
            _ => None,
        }
    }

    fn apply_self_contained(
        self,
        info: Self::SignedInfo,
    ) -> Option<sp_runtime::DispatchResultWithInfo<PostDispatchInfoOf<Self>>> {
        match self {
            call @ RuntimeCall::Ethereum(pallet_ethereum::Call::transact { .. }) => {
                Some(call.dispatch(RuntimeOrigin::from(
                    pallet_ethereum::RawOrigin::EthereumTransaction(info),
                )))
            }
            _ => None,
        }
    }
}

impl_runtime_apis! {
    impl sp_api::Core<Block> for Runtime {
        fn version() -> RuntimeVersion {
            VERSION
        }

        fn execute_block(block: Block) {
            Executive::execute_block(block);
        }

        fn initialize_block(header: &<Block as BlockT>::Header) {
            Executive::initialize_block(header)
        }
    }

    impl sp_api::Metadata<Block> for Runtime {
        fn metadata() -> OpaqueMetadata {
            OpaqueMetadata::new(Runtime::metadata().into())
        }
    }

    impl sp_block_builder::BlockBuilder<Block> for Runtime {
        fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
            Executive::apply_extrinsic(extrinsic)
        }

        fn finalize_block() -> <Block as BlockT>::Header {
            Executive::finalize_block()
        }

        fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
            data.create_extrinsics()
        }

        fn check_inherents(
            block: Block,
            data: sp_inherents::InherentData,
        ) -> sp_inherents::CheckInherentsResult {
            data.check_extrinsics(&block)
        }
    }

    impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
        fn validate_transaction(
            source: TransactionSource,
            tx: <Block as BlockT>::Extrinsic,
            block_hash: <Block as BlockT>::Hash,
        ) -> TransactionValidity {
            Executive::validate_transaction(source, tx, block_hash)
        }
    }

    impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
        fn offchain_worker(header: &<Block as BlockT>::Header) {
            Executive::offchain_worker(header)
        }
    }

    impl sp_consensus_aura::AuraApi<Block, AuraId> for Runtime {
        fn slot_duration() -> sp_consensus_aura::SlotDuration {
            sp_consensus_aura::SlotDuration::from_millis(Aura::slot_duration())
        }

        fn authorities() -> Vec<AuraId> {
            Aura::authorities().into_inner()
        }
    }

    impl sp_session::SessionKeys<Block> for Runtime {
        fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
            SessionKeys::generate(seed)
        }

        fn decode_session_keys(
            encoded: Vec<u8>,
        ) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
            SessionKeys::decode_into_raw_public_keys(&encoded)
        }
    }

    impl fg_primitives::GrandpaApi<Block> for Runtime {
        fn grandpa_authorities() -> GrandpaAuthorityList {
            Grandpa::grandpa_authorities()
        }

        fn current_set_id() -> fg_primitives::SetId {
            Grandpa::current_set_id()
        }

        fn submit_report_equivocation_unsigned_extrinsic(
            _equivocation_proof: fg_primitives::EquivocationProof<
                <Block as BlockT>::Hash,
                NumberFor<Block>,
            >,
            _key_owner_proof: fg_primitives::OpaqueKeyOwnershipProof,
        ) -> Option<()> {
            None
        }

        fn generate_key_ownership_proof(
            _set_id: fg_primitives::SetId,
            _authority_id: GrandpaId,
        ) -> Option<fg_primitives::OpaqueKeyOwnershipProof> {
            // NOTE: this is the only implementation possible since we've
            // defined our key owner proof type as a bottom type (i.e. a type
            // with no values).
            None
        }
    }

    impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Index> for Runtime {
        fn account_nonce(account: AccountId) -> Index {
            System::account_nonce(account)
        }
    }

    impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
        fn query_info(
            uxt: <Block as BlockT>::Extrinsic,
            len: u32,
        ) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
            TransactionPayment::query_info(uxt, len)
        }
        fn query_fee_details(
            uxt: <Block as BlockT>::Extrinsic,
            len: u32,
        ) -> pallet_transaction_payment::FeeDetails<Balance> {
            TransactionPayment::query_fee_details(uxt, len)
        }
    }

    impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentCallApi<Block, Balance, RuntimeCall>
        for Runtime
    {
        fn query_call_info(
            call: RuntimeCall,
            len: u32,
        ) -> pallet_transaction_payment::RuntimeDispatchInfo<Balance> {
            TransactionPayment::query_call_info(call, len)
        }
        fn query_call_fee_details(
            call: RuntimeCall,
            len: u32,
        ) -> pallet_transaction_payment::FeeDetails<Balance> {
            TransactionPayment::query_call_fee_details(call, len)
        }
    }

    impl fp_rpc::EthereumRuntimeRPCApi<Block> for Runtime {
        fn chain_id() -> u64 {
            ChainId::get()
        }

        fn account_basic(address: H160) -> pallet_evm::Account {
            let (account, _) = EVM::account_basic(&address);
            account
        }

        fn gas_price() -> U256 {
            let (gas_price, _) = <Runtime as pallet_evm::Config>::FeeCalculator::min_gas_price();
            gas_price
        }

        fn account_code_at(address: H160) -> Vec<u8> {
            EVM::account_codes(address)
        }

        fn author() -> H160 {
            <pallet_evm::Pallet<Runtime>>::find_author()
        }

        fn storage_at(address: H160, index: U256) -> H256 {
            let mut tmp = [0u8; 32];
            index.to_big_endian(&mut tmp);
            EVM::account_storages(address, H256::from_slice(&tmp[..]))
        }

        fn call(
            from: H160,
            to: H160,
            data: Vec<u8>,
            value: U256,
            gas_limit: U256,
            max_fee_per_gas: Option<U256>,
            max_priority_fee_per_gas: Option<U256>,
            nonce: Option<U256>,
            estimate: bool,
            _access_list: Option<Vec<(H160, Vec<H256>)>>,
        ) -> Result<pallet_evm::CallInfo, sp_runtime::DispatchError> {
            let config = if estimate {
                let mut config = <Runtime as pallet_evm::Config>::config().clone();
                config.estimate = true;
                Some(config)
            } else {
                None
            };

            let is_transactional = false;
            let validate = true;
            <Runtime as pallet_evm::Config>::Runner::call(
                from,
                to,
                data,
                value,
                gas_limit.unique_saturated_into(),
                max_fee_per_gas,
                max_priority_fee_per_gas,
                nonce,
                Vec::new(),
                is_transactional,
                validate,
                config
                    .as_ref()
                    .unwrap_or_else(|| <Runtime as pallet_evm::Config>::config()),
            )
            .map_err(|err| err.error.into())
        }

        fn create(
            from: H160,
            data: Vec<u8>,
            value: U256,
            gas_limit: U256,
            max_fee_per_gas: Option<U256>,
            max_priority_fee_per_gas: Option<U256>,
            nonce: Option<U256>,
            estimate: bool,
            _access_list: Option<Vec<(H160, Vec<H256>)>>,
        ) -> Result<pallet_evm::CreateInfo, sp_runtime::DispatchError> {
            let config = if estimate {
                let mut config = <Runtime as pallet_evm::Config>::config().clone();
                config.estimate = true;
                Some(config)
            } else {
                None
            };

            let is_transactional = false;
            let validate = true;
            #[allow(clippy::or_fun_call)] // suggestion not helpful here
            <Runtime as pallet_evm::Config>::Runner::create(
                from,
                data,
                value,
                gas_limit.unique_saturated_into(),
                max_fee_per_gas,
                max_priority_fee_per_gas,
                nonce,
                Vec::new(),
                is_transactional,
                validate,
                config
                    .as_ref()
                    .unwrap_or(<Runtime as pallet_evm::Config>::config()),
                )
                .map_err(|err| err.error.into())
        }

        fn current_transaction_statuses() -> Option<Vec<fp_rpc::TransactionStatus>> {
            Ethereum::current_transaction_statuses()
        }

        fn current_block() -> Option<pallet_ethereum::Block> {
            Ethereum::current_block()
        }

        fn current_receipts() -> Option<Vec<pallet_ethereum::Receipt>> {
            Ethereum::current_receipts()
        }

        fn current_all() -> (
            Option<pallet_ethereum::Block>,
            Option<Vec<pallet_ethereum::Receipt>>,
            Option<Vec<fp_rpc::TransactionStatus>>,
        ) {
            (
                Ethereum::current_block(),
                Ethereum::current_receipts(),
                Ethereum::current_transaction_statuses(),
            )
        }

        fn extrinsic_filter(
            xts: Vec<<Block as BlockT>::Extrinsic>,
        ) -> Vec<pallet_ethereum::Transaction> {
            xts.into_iter().filter_map(|xt| match xt.0.function {
                RuntimeCall::Ethereum(pallet_ethereum::Call::transact { transaction }) => Some(transaction),
                _ => None
            }).collect::<Vec<pallet_ethereum::Transaction>>()
        }

        fn elasticity() -> Option<Permill> {
            Some(BaseFee::elasticity())
        }
    }

    impl fp_rpc::ConvertTransactionRuntimeApi<Block> for Runtime {
        fn convert_transaction(
            transaction: pallet_ethereum::Transaction
        ) -> <Block as BlockT>::Extrinsic {
            UncheckedExtrinsic::new_unsigned(
                pallet_ethereum::Call::<Runtime>::transact { transaction }.into(),
            )
        }
    }

    impl pallet_contracts_rpc_runtime_api::ContractsApi<
        Block, AccountId, Balance, BlockNumber, Hash,
    >
        for Runtime
    {
        fn call(
            origin: AccountId,
            dest: AccountId,
            value: Balance,
            gas_limit: u64,
            storage_deposit_limit: Option<Balance>,
            input_data: Vec<u8>,
        ) -> pallet_contracts_primitives::ContractExecResult<Balance> {
            Contracts::bare_call(origin, dest, value, Weight::from_ref_time(gas_limit), storage_deposit_limit, input_data, true)
        }

        fn instantiate(
            origin: AccountId,
            value: Balance,
            gas_limit: u64,
            storage_deposit_limit: Option<Balance>,
            code: pallet_contracts_primitives::Code<Hash>,
            data: Vec<u8>,
            salt: Vec<u8>,
        ) -> pallet_contracts_primitives::ContractInstantiateResult<AccountId, Balance>
        {
            Contracts::bare_instantiate(origin, value, Weight::from_ref_time(gas_limit), storage_deposit_limit, code, data, salt, true)
        }

        fn upload_code(
            origin: AccountId,
            code: Vec<u8>,
            storage_deposit_limit: Option<Balance>,
        ) -> pallet_contracts_primitives::CodeUploadResult<Hash, Balance>
        {
            Contracts::bare_upload_code(origin, code, storage_deposit_limit)
        }

        fn get_storage(
            address: AccountId,
            key: Vec<u8>,
        ) -> pallet_contracts_primitives::GetStorageResult {
            Contracts::get_storage(address, key)
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    impl frame_benchmarking::Benchmark<Block> for Runtime {
        fn benchmark_metadata(extra: bool) -> (
            Vec<frame_benchmarking::BenchmarkList>,
            Vec<frame_support::traits::StorageInfo>,
        ) {
            use frame_benchmarking::{list_benchmark, Benchmarking, BenchmarkList};
            use frame_support::traits::StorageInfoTrait;

            let mut list = Vec::<BenchmarkList>::new();

            list_benchmark!(list, extra, pallet_dapps_staking, DappsStaking);
            list_benchmark!(list, extra, pallet_dapps_staking, BlockReward);
            list_benchmark!(list, extra, pallet_balances, Balances);

            let storage_info = AllPalletsWithSystem::storage_info();

            (list, storage_info)
        }

        fn dispatch_benchmark(
            config: frame_benchmarking::BenchmarkConfig
        ) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, sp_runtime::RuntimeString> {
            use frame_benchmarking::{Benchmarking, BenchmarkBatch, add_benchmark, TrackedStorageKey};

            use frame_system_benchmarking::Pallet as SystemBench;
            impl frame_system_benchmarking::Config for Runtime {}

            let whitelist: Vec<TrackedStorageKey> = vec![
                // Block Number
                array_bytes::hex_into_unchecked("26aa394eea5630e07c48ae0c9558cef702a5c1b19ab7a04f536c519aca4983ac"),
                // Execution Phase
                array_bytes::hex_into_unchecked("26aa394eea5630e07c48ae0c9558cef7ff553b5a9862a516939d82b3d3d8661a"),
                // Event Count
                array_bytes::hex_into_unchecked("26aa394eea5630e07c48ae0c9558cef70a98fdbe9ce6c55837576c60c7af3850"),
                // System Events
                array_bytes::hex_into_unchecked("26aa394eea5630e07c48ae0c9558cef780d41e5e16056765bc8461851072c9d7"),
            ];

            let mut batches = Vec::<BenchmarkBatch>::new();
            let params = (&config, &whitelist);

            add_benchmark!(params, batches, frame_system, SystemBench::<Runtime>);
            add_benchmark!(params, batches, pallet_balances, Balances);
            add_benchmark!(params, batches, pallet_timestamp, Timestamp);
            add_benchmark!(params, batches, pallet_dapps_staking, DappsStaking);
            add_benchmark!(params, batches, pallet_block_reward, BlockReward);

            if batches.is_empty() { return Err("Benchmark not found for this pallet.".into()) }
            Ok(batches)
        }
    }

    #[cfg(feature = "try-runtime")]
    impl frame_try_runtime::TryRuntime<Block> for Runtime {
        fn on_runtime_upgrade() -> (Weight, Weight) {
            log::info!("try-runtime::on_runtime_upgrade");
            let weight = Executive::try_runtime_upgrade().unwrap();
            (weight, RuntimeBlockWeights::get().max_block)
        }

        fn execute_block(
            block: Block,
            state_root_check: bool,
            select: frame_try_runtime::TryStateSelect
        ) -> Weight {
            log::info!(
                "try-runtime: executing block #{} ({:?}) / root checks: {:?} / sanity-checks: {:?}",
                block.header.number,
                block.header.hash(),
                state_root_check,
                select,
            );
            Executive::try_execute_block(block, state_root_check, select).expect("execute-block failed")
        }
    }
}
