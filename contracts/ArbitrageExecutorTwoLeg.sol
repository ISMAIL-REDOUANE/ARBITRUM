// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title ArbitrageExecutorTwoLeg
/// @notice Two-leg arbitrage executor with Balancer V2 flash loan
/// @dev Supports: TokenA -> DEX1 -> TokenB -> DEX2 -> TokenA
/// @dev Based on ArbitrageExecutorYul architecture, extended for two legs
contract ArbitrageExecutorTwoLeg {
    /// @notice Balancer V2 Vault
    address constant BALANCER_VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;

    /// @notice Uniswap V3 Router
    address constant UNISWAP_V3_ROUTER = 0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45;

    /// @notice WETH
    address constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;

    /// @notice Minimum profit threshold (wei)
    uint256 public minProfit;

    /// @notice Owner
    address public owner;

    /// @notice Arbitrage profit event
    event Profit(uint256 profit, uint256 gasLeft);

    /// @notice Flash loan received
    event LoanReceived(address token, uint256 amount, uint256 fee);

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

    /// @notice Entry point - executes two-leg arbitrage via Balancer flash loan
    /// @param loanToken Token to borrow
    /// @param loanAmount Amount to borrow
    /// @param leg1Pool Pool for first swap (TokenA -> TokenB)
    /// @param leg2Pool Pool for second swap (TokenB -> TokenA)
    /// @param leg1Data Encoded first swap parameters
    /// @param leg2Data Encoded second swap parameters
    /// @return profit Net profit in wei
    function execute(
        address loanToken,
        uint256 loanAmount,
        address leg1Pool,
        address leg2Pool,
        bytes calldata leg1Data,
        bytes calldata leg2Data
    ) external returns (uint256 profit) {
        require(leg1Pool != address(0), "INVALID_LEG1_POOL");
        require(leg2Pool != address(0), "INVALID_LEG2_POOL");

        bytes memory userData = abi.encode(
            address(this),  // initiator
            loanToken,
            loanAmount,
            leg1Pool,
            leg2Pool,
            leg1Data,
            leg2Data
        );

        IERC20[] memory tokens = new IERC20[](1);
        tokens[0] = IERC20(loanToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = loanAmount;

        (bool success, bytes memory returnData) = BALANCER_VAULT.call(
            abi.encodeWithSelector(
                bytes4(keccak256("flashLoan(address,address[],uint256[],bytes)")),
                tokens, amounts, userData
            )
        );

        if (!success) revert(_getRevertReason(returnData));
        profit = IERC20(loanToken).balanceOf(address(this));
    }

    /// @notice Balancer callback - executes two-leg arbitrage
    /// @dev This is called by Balancer Vault after sending the flash loan
    function receiveFlashLoan(
        address[] calldata tokens,
        uint256[] calldata amounts,
        uint256[] calldata feeAmounts,
        bytes calldata userData
    ) external {
        assembly {
            // ═══════════════════════════════════════════════════════════════
            // GUARD: Verify caller is Balancer Vault
            // ═══════════════════════════════════════════════════════════════
            if iszero(eq(caller(), BALANCER_VAULT)) {
                revert(0, 0)
            }

            // ═══════════════════════════════════════════════════════════════
            // PARSE USERDATA
            // userData layout:
            // 0x00: address (initiator)
            // 0x20: address (loanToken)
            // 0x40: uint256 (loanAmount)
            // 0x60: address (leg1Pool)
            // 0x80: address (leg2Pool)
            // 0xa0: bytes (leg1Data) offset
            // 0xc0: bytes (leg1Data) length
            // 0xe0: bytes (leg2Data) offset
            // 0x100: bytes (leg2Data) length
            // ═══════════════════════════════════════════════════════════════

            let loanToken := calldataload(0x20)
            let loanAmount := calldataload(0x40)
            let leg1Pool := calldataload(0x60)
            let leg2Pool := calldataload(0x80)
            let leg1DataOffset := calldataload(0xa0)
            let leg1DataLen := calldataload(add(0xa0, 0x20))
            let leg2DataOffset := calldataload(0xe0)
            let leg2DataLen := calldataload(add(0xe0, 0x20))

            // ═══════════════════════════════════════════════════════════════
            // GET BALANCE BEFORE SWAPS
            // ═══════════════════════════════════════════════════════════════
            mstore(0x00, loanToken)
            mstore(0x20, 0)
            pop(staticcall(gas(), loanToken, 0x00, 0x40, 0x00, 0x20))
            let balBefore := mload(0x00)

            // ═══════════════════════════════════════════════════════════════
            // APPROVE ROUTER FOR LOAN TOKEN
            // ═══════════════════════════════════════════════════════════════
            mstore(0x00, 0x095ea7b300000000000000000000000000000000000000000000000000000000)
            mstore(0x04, UNISWAP_V3_ROUTER)
            mstore(0x24, loanAmount)
            pop(call(gas(), loanToken, 0, 0x00, 0x44, 0x00, 0x00))

            // ═══════════════════════════════════════════════════════════════
            // LEG 1: Token -> Token via leg1Pool
            // leg1Data: (tokenIn, tokenOut, fee, recipient, amountIn, amountOutMin, sqrtPriceLimit)
            // ═══════════════════════════════════════════════════════════════
            let leg1Success := call(
                gas(),
                UNISWAP_V3_ROUTER,
                0,
                leg1DataOffset,
                leg1DataLen,
                0x00,
                0x20
            )

            if iszero(leg1Success) { revert(0, 0) }

            // ═══════════════════════════════════════════════════════════════
            // LEG 2: Token -> Token via leg2Pool
            // Use output of leg1 as input for leg2
            // leg2Data: (tokenIn, tokenOut, fee, recipient, amountIn, amountOutMin, sqrtPriceLimit)
            // ═══════════════════════════════════════════════════════════════
            let leg2Success := call(
                gas(),
                UNISWAP_V3_ROUTER,
                0,
                leg2DataOffset,
                leg2DataLen,
                0x00,
                0x20
            )

            if iszero(leg2Success) { revert(0, 0) }

            // ═══════════════════════════════════════════════════════════════
            // GET FINAL BALANCE
            // ═══════════════════════════════════════════════════════════════
            mstore(0x00, loanToken)
            mstore(0x20, 0)
            pop(staticcall(gas(), loanToken, 0x00, 0x40, 0x00, 0x20))
            let balAfter := mload(0x00)

            // ═══════════════════════════════════════════════════════════════
            // CALCULATE REPAYMENT: principal + feeAmounts
            // ═══════════════════════════════════════════════════════════════
            let feeAmount := calldataload(add(0x20, 0x60)) // feeAmounts[0]
            let repayment := add(loanAmount, feeAmount)

            // ═══════════════════════════════════════════════════════════════
            // VERIFY PROFIT
            // profit = balAfter - repayment
            // profit must be >= minProfit
            // ═══════════════════════════════════════════════════════════════
            let profit := sub(balAfter, repayment)
            let storedMinProfit := sload(minProfit.slot)
            if lt(profit, storedMinProfit) { revert(0, 0) }

            // ═══════════════════════════════════════════════════════════════
            // REPAY FLASH LOAN: principal + fees
            // ═══════════════════════════════════════════════════════════════
            mstore(0x00, 0xa9059cbb00000000000000000000000000000000000000000000000000000000)
            mstore(0x04, BALANCER_VAULT)
            mstore(0x24, repayment)
            pop(call(gas(), loanToken, 0, 0x00, 0x44, 0x00, 0x00))

            // ═══════════════════════════════════════════════════════════════
            // SEND PROFIT TO INITIATOR
            // ═══════════════════════════════════════════════════════════════
            let initiator := calldataload(0x00)
            mstore(0x00, 0xa9059cbb00000000000000000000000000000000000000000000000000000000)
            mstore(0x04, initiator)
            mstore(0x24, profit)
            pop(call(gas(), loanToken, 0, 0x00, 0x44, 0x00, 0x00))

            // ═══════════════════════════════════════════════════════════════
            // EMIT PROFIT EVENT
            // ═══════════════════════════════════════════════════════════════
            mstore(0x00, profit)
            mstore(0x20, gas())
            log2(0x00, 0x40, 0x0000000000000000000000000000000000000000000000000000000000000001, 0x00)
        }
    }

    function rescueToken(address token, uint256 amount) external onlyOwner {
        (bool success,) = token.call(abi.encodeWithSelector(IERC20.transfer.selector, owner, amount));
        require(success, "TRANSFER_FAILED");
    }

    function rescueETH() external onlyOwner {
        (bool success,) = owner.call{value: address(this).balance}("");
        require(success, "ETH_TRANSFER_FAILED");
    }

    function _getRevertReason(bytes memory data) internal pure returns (string memory) {
        if (data.length == 0) return "REVERT";
        if (data.length < 68) return "REVERT_SHORT";
        assembly {
            data := add(data, 4)
            let returndata_size := sub(mload(data), 32)
            data := add(data, 32)
            if gt(returndata_size, 32) { returndata_size := 32 }
            returndatacopy(0, data, returndata_size)
            revert(0, add(returndata_size, 32))
        }
    }
}

interface IERC20 {
    function balanceOf(address) external view returns (uint256);
    function transfer(address, uint256) external returns (bool);
    function approve(address, uint256) external returns (bool);
}
