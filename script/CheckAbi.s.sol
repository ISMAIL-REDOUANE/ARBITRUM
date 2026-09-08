// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "forge-std/console.sol";

contract CheckAbi is Script {
    struct Params {
        address tokenIn;
        address tokenOut;
        uint24 fee;
        address recipient;
        uint256 amountIn;
        uint256 amountOutMinimum;
        uint160 sqrtPriceLimitX96;
    }

    function run() external {
        bytes memory data = abi.encodeWithSelector(
            bytes4(keccak256("exactInputSingle((address,address,uint24,address,uint256,uint256,uint160))")),
            Params(address(0x1111), address(0x2222), 300, address(0x4444), 5, 6, 7)
        );
        console.log("Calldata length:", data.length);
        console.logBytes(data);
    }
}
