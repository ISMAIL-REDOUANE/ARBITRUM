// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

contract ArbitrageExecutorYulV2 {
    address constant BALANCER_VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;
    address constant UNISWAP_V3_ROUTER = 0xE592427A0AEce92De3Edee1F18E0157C05861564;
    address constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;
    address constant USDC = 0xaf88d065e77c8cC2239327C5EDb3A432268e5831;

    uint256 public minProfit;
    address public owner;

    event Profit(uint256 profit, uint256 gasLeft);

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

    function execute(
        address loanToken,
        uint256 loanAmount,
        uint256 minProfitAmount,
        address pool,
        address tokenOut,
        uint24 fee,
        address recipient
    ) external returns (uint256 profit) {
        require(minProfitAmount >= minProfit, "Min profit too low");

        bytes memory userData = abi.encode(
            address(this), loanToken, loanAmount, minProfitAmount, pool, tokenOut, fee, recipient
        );

        IERC20[] memory tokens = new IERC20[](1);
        tokens[0] = IERC20(loanToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = loanAmount;

        (bool success, bytes memory returnData) = BALANCER_VAULT.call(
            abi.encodeWithSelector(
                bytes4(keccak256("flashLoan(address[],uint256[],bytes)")),
                tokens, amounts, userData
            )
        );

        if (!success) revert(_getRevertReason(returnData));
        profit = IERC20(loanToken).balanceOf(address(this));
    }

    function receiveFlashLoan(
        address[] calldata tokens,
        uint256[] calldata amounts,
        uint256[] calldata feeAmounts,
        bytes calldata userData
    ) external {
        require(msg.sender == BALANCER_VAULT, "CB1");
        require(tokens.length == 1 && amounts.length == 1 && feeAmounts.length == 1, "CB2");

        address loanToken = tokens[0];
        uint256 loanAmount = amounts[0];
        uint256 feeAmount = feeAmounts[0];

        address initiator;
        uint256 minProfitAmount;
        address tokenOut;
        uint24 fee;
        address recipient;

        assembly ("memory-safe") {
            initiator := calldataload(add(userData.offset, 4))
            minProfitAmount := calldataload(add(userData.offset, 36))
            tokenOut := calldataload(add(userData.offset, 68))
            fee := calldataload(add(userData.offset, 100))
            recipient := calldataload(add(userData.offset, 132))
        }

        _safeApprove(loanToken, UNISWAP_V3_ROUTER, loanAmount);

        bytes memory swapParams = abi.encode(
            loanToken, tokenOut, fee, recipient, block.timestamp + 60,
            loanAmount, 0, 0
        );

        (bool swapSuccess, bytes memory swapData) = UNISWAP_V3_ROUTER.call(
            abi.encodeWithSelector(
                bytes4(keccak256("exactInputSingle((address,address,uint24,address,uint256,uint256,uint256,uint256)")),
                swapParams
            )
        );

        if (!swapSuccess) revert(_getRevertReason(swapData));

        uint256 balAfter = IERC20(loanToken).balanceOf(address(this));
        uint256 repayment = loanAmount + feeAmount;

        require(balAfter > repayment, "CB5");
        uint256 profit = balAfter - repayment;
        require(profit >= minProfitAmount, "CB6");

        _safeApprove(loanToken, BALANCER_VAULT, repayment);
        _safeTransfer(loanToken, BALANCER_VAULT, repayment);

        if (profit > 0) _safeTransfer(loanToken, initiator, profit);

        emit Profit(profit, gasleft());
    }

    function rescueToken(address token, uint256 amount) external onlyOwner {
        _safeTransfer(token, owner, amount);
    }

    function rescueETH() external onlyOwner {
        (bool success,) = owner.call{value: address(this).balance}("");
        require(success, "ETH transfer failed");
    }

    function _safeApprove(address token, address spender, uint256 amount) internal {
        (bool success, bytes memory data) = token.call(
            abi.encodeWithSelector(IERC20.approve.selector, spender, amount)
        );
        require(success && (data.length == 0 || abi.decode(data, (bool))), "APPROVE_FAILED");
    }

    function _safeTransfer(address token, address to, uint256 amount) internal {
        (bool success, bytes memory data) = token.call(
            abi.encodeWithSelector(IERC20.transfer.selector, to, amount)
        );
        require(success && (data.length == 0 || abi.decode(data, (bool))), "TRANSFER_FAILED");
    }

    function _getRevertReason(bytes memory data) internal pure returns (string memory) {
        if (data.length == 0) return "REVERT";
        if (data.length < 68) return "REVERT_SHORT";
        assembly ("memory-safe") {
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
