// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../contracts/ArbitrageExecutorYulV2.sol";

contract ArbitrageExecutorV2Test is Test {
    ArbitrageExecutorYulV2 public executor;
    address public owner;
    address public user;

    MockBalancerVault public mockBalancer;

    address constant USDC = 0xaf88d065e77c8cC2239327C5EDb3A432268e5831;
    address constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;
    address constant BALANCER_VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;

    function setUp() public {
        owner = address(this);
        user = makeAddr("user");
        mockBalancer = new MockBalancerVault();
        executor = new ArbitrageExecutorYulV2(0);
    }

    function testOwnerInitialization() public view {
        assertEq(executor.owner(), address(this));
        assertEq(executor.minProfit(), 0);
    }

    function testSetMinProfitOwner() public {
        vm.prank(owner);
        executor.setMinProfit(1_000_000);
        assertEq(executor.minProfit(), 1_000_000);
    }

    function testSetMinProfitNonOwner() public {
        vm.prank(user);
        vm.expectRevert();
        executor.setMinProfit(1_000_000);
    }

    function testFlashLoanSelector() public pure {
        bytes4 selector = bytes4(keccak256("flashLoan(address[],uint256[],bytes)"));
        assertEq(uint32(selector), uint32(0x788fb484));
    }

    function testUserDataEncoding() public pure {
        bytes memory userData = abi.encode(
            address(0x123),
            USDC,
            1000e6,
            100e6,
            address(0x456),
            address(0x789),
            uint24(3000),
            address(0xABC)
        );
        assertEq(userData.length, 256);
    }

    function testRescueToken() public {
        MockERC20 usdc = new MockERC20();
        usdc.mint(address(executor), 1000e6);

        vm.prank(owner);
        executor.rescueToken(address(usdc), 1000e6);

        assertEq(usdc.balanceOf(owner), 1000e6);
    }

    function testRescueTokenNonOwner() public {
        MockERC20 usdc = new MockERC20();
        usdc.mint(address(executor), 1000e6);

        vm.prank(user);
        vm.expectRevert();
        executor.rescueToken(address(usdc), 1000e6);
    }

    function testProfitCalculationUnderflowProtection() public pure {
        uint256 balAfter = 1000e6;
        uint256 loanAmount = 1100e6;
        uint256 feeAmount = 1e6;
        uint256 repayment = loanAmount + feeAmount;
        assertTrue(balAfter <= repayment);
    }

    function testProfitCalculationSuccess() public pure {
        uint256 balAfter = 2000e6;
        uint256 loanAmount = 1000e6;
        uint256 feeAmount = 1e6;
        uint256 repayment = loanAmount + feeAmount;
        uint256 profit = balAfter - repayment;
        assertEq(profit, 999e6);
    }
}

contract MockBalancerVault {
    bytes4 constant CALLBACK_SELECTOR = 0x8a1e5e83;

    function flashLoan(
        address[] memory tokens,
        uint256[] memory amounts,
        bytes memory userData
    ) external {
        require(tokens.length == 1);
        require(amounts.length == 1);

        uint256 fee = amounts[0] / 1000;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = fee;

        IFlashLoanRecipient(msg.sender).receiveFlashLoan(
            tokens, amounts, feeAmounts, userData
        );
    }
}

interface IFlashLoanRecipient {
    function receiveFlashLoan(
        address[] calldata tokens,
        uint256[] calldata amounts,
        uint256[] calldata feeAmounts,
        bytes calldata userData
    ) external;
}

contract MockERC20 {
    mapping(address => uint256) public balanceOf;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount);
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function approve(address, uint256) external pure returns (bool) {
        return true;
    }
}
