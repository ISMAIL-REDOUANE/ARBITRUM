// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../contracts/ArbitrageExecutorTwoLeg.sol";

interface IUniswapV3Factory {
    function getPool(address tokenA, address tokenB, uint24 fee) external view returns (address pool);
}

contract ArbitrumMainnetForkTest is Test {
    ArbitrageExecutorTwoLeg public executor;

    address constant BALANCER_VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;
    address constant UNISWAP_V3_ROUTER = 0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45;
    address constant UNISWAP_V3_FACTORY = 0x1F98431c8aD98523631AE4a59f267346ea31F984;
    address constant USDC = 0xaf88d065e77c8cC2239327C5EDb3A432268e5831;
    address constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;

    function setUp() public {
        string memory rpcUrl = vm.envOr("TRUSTED_ARBITRUM_RPC", string(""));
        if (bytes(rpcUrl).length == 0) {
            return; // Skip fork setup if env var is missing
        }

        vm.createSelectFork(rpcUrl);

        // Verify chain ID is Arbitrum (42161)
        assertEq(block.chainid, 42161, "Chain ID must be 42161");

        // Fail-closed bytecode verification
        require(BALANCER_VAULT.code.length > 0, "No code at Balancer Vault");
        require(UNISWAP_V3_ROUTER.code.length > 0, "No code at Uniswap V3 Router");
        require(USDC.code.length > 0, "No code at USDC");
        require(WETH.code.length > 0, "No code at WETH");

        executor = new ArbitrageExecutorTwoLeg(1); // minProfit = 1 unit
    }

    function testFork_ArbitrumDeploymentVerify() public {
        string memory rpcUrl = vm.envOr("TRUSTED_ARBITRUM_RPC", string(""));
        if (bytes(rpcUrl).length == 0) {
            return;
        }
        assertTrue(address(executor) != address(0));
    }

    function testFork_UnreachableMinProfitReverts() public {
        string memory rpcUrl = vm.envOr("TRUSTED_ARBITRUM_RPC", string(""));
        if (bytes(rpcUrl).length == 0) {
            return;
        }

        // Deploy with an unreachable minProfit of 1 million USDC
        ArbitrageExecutorTwoLeg highProfitExecutor = new ArbitrageExecutorTwoLeg(1000000000000);

        // Build valid calldata structures but for unviable route
        bytes memory leg1Data = _buildLegData(USDC, WETH, address(highProfitExecutor), 500, 10000000, 1, 0);
        bytes memory leg2Data = _buildLegData(WETH, USDC, address(highProfitExecutor), 3000, 1, 1, 0);

        vm.expectRevert();
        highProfitExecutor.execute(
            USDC,
            10000000, // 10 USDC loan
            UNISWAP_V3_ROUTER,
            UNISWAP_V3_ROUTER,
            leg1Data,
            leg2Data
        );
    }

    /// @notice Full execution profitable route test on Arbitrum Mainnet Fork
    /// @dev Only CONTRACTS are real; spread is injected via prank-funding/reserves to test end-to-end execution.
    function testFork_FullExecution_ProfitableRoute() public {
        string memory rpcUrl = vm.envOr("TRUSTED_ARBITRUM_RPC", string(""));
        if (bytes(rpcUrl).length == 0) {
            return;
        }

        assertEq(block.chainid, 42161, "Chain ID must be 42161");
        vm.deal(address(this), 10 ether);

        address pool500 = IUniswapV3Factory(UNISWAP_V3_FACTORY).getPool(WETH, USDC, 500);
        address pool3000 = IUniswapV3Factory(UNISWAP_V3_FACTORY).getPool(WETH, USDC, 3000);
        require(pool500 != address(0) && pool3000 != address(0), "Pools not found");

        // Pre-existing balance exclusion test
        uint256 preExistingExtraWETH = 0.5 ether;
        deal(WETH, address(executor), preExistingExtraWETH);

        // Dislocation: Shift pool500 slot0 sqrtPriceX96 to create profitable arbitrage spread
        bytes32 slot0Val = vm.load(pool500, bytes32(uint256(0)));
        uint160 currentSqrtPrice = uint160(uint256(slot0Val));
        uint160 modifiedSqrtPrice = currentSqrtPrice * 105 / 100;
        bytes32 newSlot0Val = bytes32((uint256(slot0Val) & ~uint256(type(uint160).max)) | uint256(modifiedSqrtPrice));
        vm.store(pool500, bytes32(uint256(0)), newSlot0Val);

        uint256 principal = 0.01 ether; // 18 decimals (WETH)
        uint160 limitLeg1 = currentSqrtPrice * 99 / 100; // Non-zero valid slippage bound (< currentSqrtPrice)
        uint160 limitLeg2 = currentSqrtPrice * 101 / 100; // Non-zero valid slippage bound (> currentSqrtPrice)

        // Leg 1: WETH (18 dec) -> USDC (6 dec). 0.01 WETH -> min 10 USDC (10_000_000 raw)
        bytes memory leg1Data = _buildLegData(WETH, USDC, address(executor), 500, principal, 10_000_000, limitLeg1);
        // Leg 2: USDC (6 dec) -> WETH (18 dec). ~25 USDC -> min principal + 1 wei WETH
        bytes memory leg2Data = _buildLegData(USDC, WETH, address(executor), 3000, 10_000_000, principal + 1, limitLeg2);

        vm.expectCall(
            BALANCER_VAULT,
            abi.encodeWithSelector(bytes4(keccak256("flashLoan(address,address[],uint256[],bytes)")))
        );
        vm.expectCall(
            UNISWAP_V3_ROUTER,
            abi.encodeWithSelector(bytes4(0x04e45aaf))
        );

        // Execute arbitrage - without try/catch to enforce strict pass/fail visibility
        uint256 profit = executor.execute(WETH, principal, pool500, pool3000, leg1Data, leg2Data);
        assertTrue(profit >= 1, "Profit >= minProfit");
        uint256 executorWethAfter = IERC20(WETH).balanceOf(address(executor));
        assertEq(executorWethAfter, preExistingExtraWETH + profit, "Pre-existing balance excluded");
    }

    function testFork_VaultFlashLoanMinimalBorrower() public {
        string memory rpcUrl = vm.envOr("TRUSTED_ARBITRUM_RPC", string(""));
        if (bytes(rpcUrl).length == 0) {
            return;
        }
        MinimalFlashLoanBorrower borrower = new MinimalFlashLoanBorrower();
        uint256 borrowAmount = 0.01 ether;
        deal(WETH, address(borrower), 0.001 ether); // for fee
        borrower.initiateFlashLoan(WETH, borrowAmount);
        assertTrue(address(borrower) != address(0));
    }

    function _buildLegData(
        address tokenIn,
        address tokenOut,
        address recipient,
        uint24 fee,
        uint256 amountIn,
        uint256 amountOutMin,
        uint160 sqrtPriceLimitX96
    ) internal pure returns (bytes memory) {
        bytes memory legData = new bytes(228);
        assembly {
            let p := add(legData, 32)
            mstore(p, shl(224, 0x04e45aaf)) // 4 bytes selector
            mstore(add(p, 4), tokenIn)      // 32 bytes (tokenIn)
            mstore(add(p, 36), tokenOut)    // 32 bytes (tokenOut)
            mstore(add(p, 68), fee)         // 32 bytes (fee)
            mstore(add(p, 100), recipient)  // 32 bytes (recipient)
            mstore(add(p, 132), amountIn)   // 32 bytes (amountIn)
            mstore(add(p, 164), amountOutMin) // 32 bytes (amountOutMin)
            mstore(add(p, 196), sqrtPriceLimitX96) // 32 bytes (sqrtPriceLimit)
        }
        return legData;
    }
}

contract MinimalFlashLoanBorrower {
    address constant BALANCER_VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;

    function initiateFlashLoan(address token, uint256 amount) external {
        IERC20[] memory tokens = new IERC20[](1);
        tokens[0] = IERC20(token);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = amount;
        (bool success, bytes memory returnData) = BALANCER_VAULT.call(
            abi.encodeWithSelector(
                bytes4(keccak256("flashLoan(address,address[],uint256[],bytes)")),
                address(this), tokens, amounts, ""
            )
        );
        require(success, string(returnData));
    }

    function receiveFlashLoan(
        address[] calldata tokens,
        uint256[] calldata amounts,
        uint256[] calldata feeAmounts,
        bytes calldata
    ) external {
        require(msg.sender == BALANCER_VAULT, "NOT_VAULT");
        uint256 repayment = amounts[0] + feeAmounts[0];
        require(IERC20(tokens[0]).transfer(BALANCER_VAULT, repayment), "REPAY_FAIL");
    }
}

