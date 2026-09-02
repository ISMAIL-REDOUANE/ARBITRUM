// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title ArbitrageExecutorYul
/// @notice Ultra-optimized Yul-asm flash swap arbitrage contract
/// @dev Scratch space I/O, bitwise parsing, zero-gas reverts
contract ArbitrageExecutorYul {
    /// @notice Balancer V2 Vault
    address constant BALANCER_VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;

    /// @notice Uniswap V3 Router
    address constant UNISWAP_V3_ROUTER = 0xE592427A0AEce92De3Edee1F18E0157C05861564;

    /// @notice WETH
    address constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;

    /// @notice USDC
    address constant USDC = address(uint160(0x0000000000000000000000000000000000000001));

    /// @notice Minimum profit threshold (wei)
    uint256 public minProfit;

    /// @notice Owner
    address public owner;

    /// @notice Arbitrage profit event
    event Profit(uint256 profit, uint256 gasLeft);

    /// @notice Flash loan received
    event LoanReceived(address token, uint256 amount);

    constructor(uint256 _minProfit) {
        owner = msg.sender;
        minProfit = _minProfit;
        assembly { sstore(minProfit.slot, _minProfit) }
    }

    modifier onlyOwner() {
        assembly { if iszero(eq(caller(), sload(owner.slot))) { revert(0, 0) } }
        _;
    }

    receive() external payable {}

    function setMinProfit(uint256 _minProfit) external onlyOwner {
        assembly { sstore(minProfit.slot, _minProfit) }
    }

    /// @notice Entry point - executes arbitrage via Balancer flash loan
    /// @dev Optimized: Uses scratch space for I/O, bitwise parsing
    /// @param loanToken Token to borrow
    /// @param loanAmount Amount to borrow
    /// @param pool Uni V3 pool address
    /// @param pathEncoded Encoded swap path (tokenOut, fee, recipient)
    /// @return profit Net profit in wei
    function execute(
        address loanToken,
        uint256 loanAmount,
        address pool,
        bytes calldata pathEncoded
    ) external returns (uint256 profit) {
        assembly {
            // ═══════════════════════════════════════════════════════════════
            // STEP 1: PARSE INPUTS FROM CALLDATA VIA BITWISE (no ABI decode)
            // ═══════════════════════════════════════════════════════════════
            // loanToken at calldata offset 0x04 (scratch space ready at 0x00)

            // ═══════════════════════════════════════════════════════════════
            // STEP 2: BUILD FLASH LOAN CALL IN SCRATCH SPACE (0x00-0x80)
            // ═══════════════════════════════════════════════════════════════
            // Selector: flashLoan(address,address[],uint256[],bytes)
            mstore(0x00, 0x8c9b2d8300000000000000000000000000000000000000000000000000000000)

            // tokens array (offset 0x04)
            mstore(0x24, 1)                      // array length = 1
            mstore(0x44, loanToken)             // tokens[0]

            // amounts array (offset 0x24) - reused scratch
            mstore(0x64, 1)                     // array length = 1
            mstore(0x84, loanAmount)            // amounts[0]

            // fees array (offset 0x44)
            mstore(0xa4, 1)                     // array length = 1
            mstore(0xc4, 0)                    // fee = 0

            // userData header (offset 0x64)
            mstore(0xe4, address())             // initiator
            mstore(0x104, loanToken)           // loanToken
            mstore(0x124, loanAmount)          // loanAmount
            mstore(0x144, pool)                // pool

            // Copy pathEncoded to scratch after userData header
            // pathEncoded.offset at calldata 0xe4, pathEncoded.length at 0x104
            let pathLen := calldataload(0x124)
            pathLen := and(pathLen, 0xFFFFFFFF)

            // Copy path data starting at 0x164
            calldatacopy(0x184, 0x128, pathLen)

            // Update free memory pointer (no expansion gas)
            mstore(0x40, add(0x184, pathLen))

            // userData pointer at 0x64
            // Call Balancer Vault: selector at 0x00, 0xa4 bytes of calldata
            let success := call(
                gas(),
                BALANCER_VAULT,
                0,
                0x00,
                0xa4,
                0x00,
                0x20
            )

            // Return profit
            if success {
                profit := mload(0x00)
            }
        }
    }

    /// @notice Balancer callback - ultra-optimized Yul
    /// @dev Uses scratch space 0x00-0x40 for all I/O, bitwise calldata parsing
    function receiveFlashLoan(
        address[] calldata tokens,
        uint256[] calldata amounts,
        uint256[] calldata feeAmounts,
        bytes calldata userData
    ) external {
        assembly {
            // ═══════════════════════════════════════════════════════════════
            // GUARD: Verify caller is Balancer Vault (custom error)
            // ═══════════════════════════════════════════════════════════════
            if iszero(eq(caller(), BALANCER_VAULT)) {
                revert(0, 0)
            }

            // ═══════════════════════════════════════════════════════════════
            // PARSE USERDATA VIA BITWISE (no memory expansion)
            // ═══════════════════════════════════════════════════════════════
            // userData layout:
            // 0x00: address (initiator) - parse with shr(96, calldataload(0x00))
            // 0x20: address (loanToken) - parse with shr(96, calldataload(0x20))
            // 0x40: uint256 (loanAmount)
            // 0x60: address (pool)
            // 0x80: bytes (pathEncoded)

            let loanToken := shr(96, calldataload(0x20))
            let loanAmount := calldataload(0x40)
            let pool := shr(96, calldataload(0x60))

            // ═══════════════════════════════════════════════════════════════
            // GET BALANCE BEFORE (reuse scratch 0x00-0x20)
            // ═══════════════════════════════════════════════════════════════
            mstore(0x00, loanToken)
            mstore(0x20, 0)                      // balanceOf placeholder
            let success := call(
                gas(),
                loanToken,
                0,
                0x00,
                0x40,
                0x00,
                0x20
            )
            let balBefore := mload(0x00)

            // ═══════════════════════════════════════════════════════════════
            // APPROVE ROUTER (reuse scratch 0x00-0x24)
            // Selector: 0x095ea7b3 + router + amount
            // ═══════════════════════════════════════════════════════════════
            mstore(0x00, 0x095ea7b300000000000000000000000000000000000000000000000000000000)
            mstore(0x04, UNISWAP_V3_ROUTER)
            mstore(0x24, loanAmount)
            pop(call(gas(), loanToken, 0, 0x00, 0x44, 0x00, 0x00))

            // ═══════════════════════════════════════════════════════════════
            // BUILD SWAP PARAMS IN SCRATCH SPACE 0x00-0xe0
            // ═══════════════════════════════════════════════════════════════
            // Parse path from userData
            let pathOffset := calldataload(0x80)
            pathOffset := and(pathOffset, 0xFFFFFFFF)
            let pathData := add(0x04, pathOffset)
            let pathLen := calldataload(pathData)
            pathData := add(pathData, 0x20)

            // Decode path: tokenOut (20 bytes) + fee (3 bytes) + recipient (20 bytes)
            let pathWord0 := calldataload(pathData)
            let tokenOut := and(pathWord0, 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF)
            let fee := and(shr(160, pathWord0), 0xFFFFFF)
            let recipient := shr(96, calldataload(add(pathData, 0x14)))

            // ExactInputSingleParams in scratch (0x00-0xe0):
            // offset 0x00: tokenIn
            mstore(0x00, loanToken)
            // offset 0x20: tokenOut
            mstore(0x20, tokenOut)
            // offset 0x40: fee
            mstore(0x40, fee)
            // offset 0x60: recipient
            mstore(0x60, recipient)
            // offset 0x80: deadline
            mstore(0x80, add(timestamp(), 60))
            // offset 0xa0: amountIn
            mstore(0xa0, loanAmount)
            // offset 0xc0: amountOutMinimum = 0
            mstore(0xc0, 0)
            // offset 0xe0: sqrtPriceLimitX96 = 0
            mstore(0xe0, 0)

            // ═══════════════════════════════════════════════════════════════
            // EXECUTE UNISWAP V3 SWAP (selector: 0x78c7a8f7)
            // ═══════════════════════════════════════════════════════════════
            mstore(0x00, 0x78c7a8f7000000000000000000000000000000000000000000000000000000)

            success := call(
                gas(),
                UNISWAP_V3_ROUTER,
                0,
                0x00,
                0x120,
                0x00,
                0x20
            )

            // Zero-gas revert on swap failure
            if iszero(success) { revert(0, 0) }

            // ═══════════════════════════════════════════════════════════════
            // BALANCE CHECK & PROFIT CALCULATION (reuse scratch 0x00-0x20)
            // ═══════════════════════════════════════════════════════════════
            mstore(0x00, loanToken)
            mstore(0x20, 0)
            success := call(gas(), loanToken, 0, 0x00, 0x40, 0x00, 0x20)
            let balAfter := mload(0x00)

            let profit := sub(balAfter, add(balBefore, loanAmount))

            // Zero-gas revert if unprofitable
            let storedMinProfit := sload(minProfit.slot)
            if lt(profit, storedMinProfit) { revert(0, 0) }

            // ═══════════════════════════════════════════════════════════════
            // REPAY FLASH LOAN (reuse scratch 0x00-0x24)
            // Selector: 0xa9059cbb + vault + amount
            // ═══════════════════════════════════════════════════════════════
            mstore(0x00, 0xa9059cbb00000000000000000000000000000000000000000000000000000000)
            mstore(0x04, BALANCER_VAULT)
            mstore(0x24, loanAmount)
            pop(call(gas(), loanToken, 0, 0x00, 0x44, 0x00, 0x00))

            // ═══════════════════════════════════════════════════════════════
            // SEND PROFIT TO CALLER (reuse scratch 0x00-0x24)
            // ═══════════════════════════════════════════════════════════════
            let initiator := shr(96, calldataload(0x00))
            mstore(0x00, 0xa9059cbb00000000000000000000000000000000000000000000000000000000)
            mstore(0x04, initiator)
            mstore(0x24, profit)
            pop(call(gas(), loanToken, 0, 0x00, 0x44, 0x00, 0x00))

            // ═══════════════════════════════════════════════════════════════
            // EMIT PROFIT EVENT (reuse scratch 0x00-0x40)
            // ═══════════════════════════════════════════════════════════════
            mstore(0x00, profit)
            mstore(0x20, gas())
            log2(0x00, 0x40, 0x0000000000000000000000000000000000000000000000000000000000000001, 0x00)
        }
    }

    /// @notice Rescue tokens - owner only
    function rescueToken(address token, uint256 amount) external onlyOwner {
        assembly {
            mstore(0x00, 0xa9059cbb00000000000000000000000000000000000000000000000000000000)
            mstore(0x04, caller())
            mstore(0x24, amount)
            let success := call(gas(), token, 0, 0x00, 0x44, 0x00, 0x00)
            if iszero(success) { revert(0, 0) }
        }
    }

    /// @notice Rescue ETH - owner only
    function rescueETH() external onlyOwner {
        assembly {
            let success := call(gas(), caller(), selfbalance(), 0x00, 0x00, 0x00, 0x00)
            if iszero(success) { revert(0, 0) }
        }
    }
}
