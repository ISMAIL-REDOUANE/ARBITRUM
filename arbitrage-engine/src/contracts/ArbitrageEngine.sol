// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

interface IERC20 {
    function balanceOf(address account) external view returns (uint256);
    function transfer(address to, uint256 amount) external returns (bool);
    function allowance(address owner, address spender) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

interface IBalancerVault {
    function flashLoan(address receiverAddress, address[] memory tokens, uint256[] memory amounts, bytes memory userData) external;
}

interface IUniswapV2Router {
    function swapExactTokensForTokens(uint256 amountIn, uint256 amountOutMin, address[] calldata path, address to, uint256 deadline) external returns (uint256[] memory amounts);
}

interface IUniswapV3Router {
    function exactInputSingle(address tokenIn, address tokenOut, uint24 fee, address recipient, uint256 deadline, uint256 amountIn, uint256 amountOutMinimum, uint160 sqrtPriceLimitX96) external payable returns (uint256 amountOut);
}

library SafeERC20 {
    function safeTransfer(IERC20 token, address to, uint256 amount) internal {
        (bool success, bytes memory data) = address(token).call(abi.encodeWithSelector(IERC20.transfer.selector, to, amount));
        require(success && (data.length == 0 || abi.decode(data, (bool))), "SAFE_ERC20_FAILED");
    }
    function safeApprove(IERC20 token, address spender, uint256 amount) internal {
        (bool success, bytes memory data) = address(token).call(abi.encodeWithSelector(IERC20.approve.selector, spender, amount));
        require(success && (data.length == 0 || abi.decode(data, (bool))), "SAFE_APPROVE_FAILED");
    }
}

abstract contract ReentrancyGuard {
    uint256 private constant NOT_ENTERED = 1;
    uint256 private constant ENTERED = 2;
    uint256 private _status;
    constructor() { _status = NOT_ENTERED; }
    modifier nonReentrant() {
        require(_status != ENTERED, "REENTRANCY_GUARD");
        _status = ENTERED;
        _;
        _status = NOT_ENTERED;
    }
}

contract ArbitrageEngine is ReentrancyGuard {
    using SafeERC20 for IERC20;

    address public owner;
    address public pendingOwner;
    uint256 public minProfitThreshold;
    uint256 public maxGasLimit;
    bool public paused;
    uint256 public immutable chainId;
    
    // Private state variables for router addresses
    address private _balancerVault;
    address private _uniswapV2Router;
    address private _uniswapV3Router;
    address private _sushiswapRouter;
    address private _aerodromeRouter;
    address private _uniswapV3Base;
    address private _uniswapV3Arbitrum;
    address private _sushiswapArbitrum;
    address private _camelotRouter;
    address private _weth;
    address private _usdc;
    address private _usdt;
    address private _wethBase;
    address private _usdcBase;
    address private _wethArb;
    address private _usdcArb;

    struct FlashLoanData {
        address[] path;
        uint256 amountIn;
        uint256 minProfit;
        uint8 dexType;
        uint256 deadline;
        address targetVault;
    }

    modifier onlyOwner() { require(msg.sender == owner, "ONLY_OWNER"); _; }
    modifier whenNotPaused() { require(!paused, "PAUSED"); _; }

    constructor() {
        owner = msg.sender;
        chainId = block.chainid;
        minProfitThreshold = 0.01 ether;
        maxGasLimit = 5_000_000;
        paused = false;
        
        // Correct EIP-55 checksummed addresses
        _balancerVault = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;
        _uniswapV2Router = 0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D;
        _uniswapV3Router = 0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45;
        _sushiswapRouter = 0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F;
        _aerodromeRouter = 0x0b404b975d461A45E3Aa6b96809746B40a76239F;
        _uniswapV3Base = 0x2626664C2603336E57B271C5C0b26d421d473758;
        _uniswapV3Arbitrum = 0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45;
        _sushiswapArbitrum = 0x1b02Da8Cb0D097CB8D57D3c6EFB1b86d8f2E2b3F;
        _camelotRouter = 0x51ec82945A4482d01B01c0C2C45D314198d2dEe2;
        _weth = 0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2;
        _usdc = 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48;
        _usdt = 0xdAC17F958D2ee523a2206206994597C13D831ec7;
        _wethBase = 0x4200000000000000000000000000000000000006;
        _usdcBase = 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913;
        _wethArb = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;
        _usdcArb = 0xaf88d065e77c8cC2239327C5EDb3A432268e5831;
    }

    receive() external payable {}
    fallback() external payable {}

    function executeArbitrage(address[] calldata tokens, uint256[] calldata amounts, address[] calldata path, uint8 dexType, uint256 minProfit) external whenNotPaused {
        require(tokens.length == amounts.length, "LENGTH_MISMATCH");
        require(path.length >= 2, "INVALID_PATH_LENGTH");
        require(minProfit >= minProfitThreshold, "BELOW_MIN_THRESHOLD");
        
        bytes memory userData = abi.encode(FlashLoanData({
            path: path,
            amountIn: amounts[0],
            minProfit: minProfit,
            dexType: dexType,
            deadline: block.timestamp + 300,
            targetVault: _balancerVault
        }));
        
        IBalancerVault(_balancerVault).flashLoan(address(this), tokens, amounts, userData);
    }

    function receiveFlashLoan(address[] memory tokens, uint256[] memory amounts, uint256[] memory feeAmounts, bytes memory userData) external nonReentrant {
        require(msg.sender == _balancerVault, "ONLY_BALANCER_VAULT");
        
        FlashLoanData memory data = abi.decode(userData, (FlashLoanData));
        
        for (uint256 i = 0; i < tokens.length; i++) {
            _approveTokenIfNeeded(tokens[i], _uniswapV2Router);
            _approveTokenIfNeeded(tokens[i], _uniswapV3Router);
            _approveTokenIfNeeded(tokens[i], _sushiswapRouter);
        }
        
        uint256 amountOut = _executeSwap(data.path, amounts[0], data.dexType);
        
        uint256 repayAmount;
        uint256 netProfit;
        bool profitOk;
        
        assembly {
            let amountBorrowed := mload(amounts)
            let feeAmount := mload(feeAmounts)
            repayAmount := add(amountBorrowed, feeAmount)
            netProfit := sub(amountOut, repayAmount)
            profitOk := gt(netProfit, 0)
            
            if iszero(profitOk) {
                mstore(0x00, 0x08c379a0)
                mstore(0x04, 0x14)
                mstore(0x24, 0x494e53554646494349454e5450524f465459525f50524f464954)
                revert(0x00, 0x44)
            }
        }
        
        require(netProfit >= data.minProfit, "BELOW_MIN_PROFIT");
        
        _sendCoinbaseBribe(netProfit / 100);
        IERC20(tokens[0]).safeTransfer(_balancerVault, repayAmount);
        
        uint256 profit = amountOut - repayAmount;
        if (profit > 0) {
            IERC20(tokens[0]).safeTransfer(msg.sender, profit);
        }
    }

    function _executeSwap(address[] memory path, uint256 amountIn, uint8 dexType) internal returns (uint256 amountOut) {
        require(path.length >= 2, "INVALID_PATH");
        
        if (dexType == 0) {
            amountOut = _swapUniswapV2(path, amountIn);
        } else if (dexType == 1) {
            amountOut = _swapUniswapV3(path, amountIn);
        } else if (dexType == 2) {
            amountOut = _swapAerodrome(path, amountIn);
        } else if (dexType == 3) {
            amountOut = _swapSushiSwap(path, amountIn);
        } else {
            revert("UNKNOWN_DEX_TYPE");
        }
        
        require(amountOut > 0, "ZERO_OUTPUT");
    }
    
    function _swapUniswapV2(address[] memory path, uint256 amountIn) internal returns (uint256 amountOut) {
        (bool success, bytes memory returnData) = _uniswapV2Router.call(
            abi.encodeWithSelector(
                bytes4(keccak256("swapExactTokensForTokens(uint256,uint256,address[],address,uint256)")),
                amountIn, 1, path, address(this), block.timestamp + 300
            )
        );
        
        if (!success) {
            if (returnData.length > 0) { revert(abi.decode(returnData, (string))); }
            revert("UNISWAP_V2_FAILED");
        }
        
        uint256[] memory amounts = abi.decode(returnData, (uint256[]));
        amountOut = amounts[amounts.length - 1];
    }
    
    function _swapUniswapV3(address[] memory path, uint256 amountIn) internal returns (uint256 amountOut) {
        (bool success, bytes memory returnData) = _uniswapV3Router.call(
            abi.encodeWithSelector(
                IUniswapV3Router.exactInputSingle.selector,
                path[0], path[1], 500, address(this), block.timestamp + 300, amountIn, 1, 0
            )
        );
        
        if (!success) {
            revert("UNISWAP_V3_FAILED");
        }
        
        amountOut = abi.decode(returnData, (uint256));
    }
    
    function _swapAerodrome(address[] memory path, uint256 amountIn) internal returns (uint256 amountOut) {
        address router = chainId == 8453 ? _aerodromeRouter : _camelotRouter;
        
        (bool success, bytes memory returnData) = router.call(
            abi.encodeWithSelector(
                bytes4(keccak256("swapExactTokensForTokens(uint256,uint256,address[],address,uint256)")),
                amountIn, 1, path, address(this), block.timestamp + 300
            )
        );
        
        if (!success) {
            if (returnData.length > 0) { revert(abi.decode(returnData, (string))); }
            revert("AERODROME_FAILED");
        }
        
        uint256[] memory amounts = abi.decode(returnData, (uint256[]));
        amountOut = amounts[amounts.length - 1];
    }
    
    function _swapSushiSwap(address[] memory path, uint256 amountIn) internal returns (uint256 amountOut) {
        address router = chainId == 42161 ? _sushiswapArbitrum : _sushiswapRouter;
        
        (bool success, bytes memory returnData) = router.call(
            abi.encodeWithSelector(
                bytes4(keccak256("swapExactTokensForTokens(uint256,uint256,address[],address,uint256)")),
                amountIn, 1, path, address(this), block.timestamp + 300
            )
        );
        
        if (!success) {
            if (returnData.length > 0) { revert(abi.decode(returnData, (string))); }
            revert("SUSHISWAP_FAILED");
        }
        
        uint256[] memory amounts = abi.decode(returnData, (uint256[]));
        amountOut = amounts[amounts.length - 1];
    }

    function _approveTokenIfNeeded(address token, address spender) internal {
        if (IERC20(token).allowance(address(this), spender) == 0) {
            IERC20(token).safeApprove(spender, type(uint256).max);
        }
    }
    
    function _sendCoinbaseBribe(uint256 amount) internal {
        if (block.coinbase != address(0) && amount > 0) {
            (bool success,) = block.coinbase.call{value: amount}("");
        }
    }
    
    function setMinProfitThreshold(uint256 newThreshold) external onlyOwner { minProfitThreshold = newThreshold; }
    function setMaxGasLimit(uint256 newLimit) external onlyOwner { require(newLimit >= 1_000_000, "GAS_TOO_LOW"); maxGasLimit = newLimit; }
    function pause() external onlyOwner { paused = true; }
    function unpause() external onlyOwner { paused = false; }
    function withdrawToken(address token, uint256 amount) external onlyOwner { IERC20(token).safeTransfer(owner, amount); }
    function withdrawETH(uint256 amount) external onlyOwner { payable(owner).transfer(amount); }
    function transferOwnership(address newOwner) external onlyOwner { require(newOwner != address(0), "ZERO_ADDRESS"); pendingOwner = newOwner; }
    function acceptOwnership() external { require(msg.sender == pendingOwner, "NOT_PENDING_OWNER"); owner = pendingOwner; pendingOwner = address(0); }
}
