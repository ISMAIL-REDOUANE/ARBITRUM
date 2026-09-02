import { useState } from 'react';
import { Shield, AlertTriangle, Zap, DollarSign } from 'lucide-react';
import { clsx } from 'clsx';

interface ControlPanelProps {
  isShadowMode: boolean;
  minProfitWei: string;
  onToggleShadowMode: () => void;
}

export function ControlPanel({ isShadowMode, minProfitWei, onToggleShadowMode }: ControlPanelProps) {
  const [localMinProfit, setLocalMinProfit] = useState(minProfitWei);
  const [relayOrder, setRelayOrder] = useState('bloxroute,flashbots,mevblocker');
  const [showRescueConfirm, setShowRescueConfirm] = useState(false);

  const handleMinProfitUpdate = () => {
    console.log('Updating min profit to:', localMinProfit);
  };

  const handleRelayOrderUpdate = () => {
    console.log('Updating relay order to:', relayOrder);
  };

  return (
    <div className="bg-cyber-panel border border-cyber-border rounded-lg p-4">
      <div className="flex items-center gap-2 mb-4">
        <Shield className="w-4 h-4 text-cyber-cyan" />
        <h2 className="text-sm font-semibold text-cyber-text tracking-wider">
          SYSTEM CONTROLS
        </h2>
      </div>

      {/* Shadow Mode Toggle */}
      <div className="mb-4">
        <label className="flex items-center justify-between cursor-pointer">
          <div className="flex items-center gap-2">
            <span className="text-xs text-cyber-muted">Shadow Mode</span>
            {isShadowMode && (
              <span className="px-2 py-0.5 text-xs bg-cyber-orange/20 text-cyber-orange rounded">
                DRY RUN
              </span>
            )}
          </div>
          <button
            onClick={onToggleShadowMode}
            className={clsx(
              'w-12 h-6 rounded-full transition-colors relative',
              isShadowMode ? 'bg-cyber-orange' : 'bg-cyber-green'
            )}
          >
            <span 
              className={clsx(
                'absolute top-1 w-4 h-4 bg-white rounded-full transition-transform',
                isShadowMode ? 'left-1' : 'left-7'
              )}
            />
          </button>
        </label>
        <p className="mt-1 text-xs text-cyber-muted">
          {isShadowMode 
            ? 'Simulation only - no live transactions'
            : 'Live mode - submitting real transactions'
          }
        </p>
      </div>

      {/* Min Profit Update */}
      <div className="mb-4 p-3 bg-cyber-dark/50 rounded border border-cyber-border">
        <div className="flex items-center gap-2 mb-2">
          <DollarSign className="w-3 h-3 text-cyber-green" />
          <span className="text-xs font-medium text-cyber-text">Min Profit Threshold</span>
        </div>
        <div className="flex gap-2">
          <input
            type="text"
            value={localMinProfit}
            onChange={(e) => setLocalMinProfit(e.target.value)}
            className="flex-1 bg-cyber-black border border-cyber-border rounded px-2 py-1 text-xs font-mono text-cyber-text focus:outline-none focus:border-cyber-green"
            placeholder="1000000000000000"
          />
          <button
            onClick={handleMinProfitUpdate}
            className="px-3 py-1 bg-cyber-green/20 text-cyber-green border border-cyber-green/30 rounded text-xs font-medium hover:bg-cyber-green/30 transition-colors"
          >
            UPDATE
          </button>
        </div>
        <p className="mt-1 text-xs text-cyber-muted">
          Current: {(parseFloat(minProfitWei) / 1e18).toFixed(4)} ETH
        </p>
      </div>

      {/* Relay Priority */}
      <div className="mb-4 p-3 bg-cyber-dark/50 rounded border border-cyber-border">
        <div className="flex items-center gap-2 mb-2">
          <Zap className="w-3 h-3 text-cyber-cyan" />
          <span className="text-xs font-medium text-cyber-text">Relay Priority Order</span>
        </div>
        <input
          type="text"
          value={relayOrder}
          onChange={(e) => setRelayOrder(e.target.value)}
          className="w-full bg-cyber-black border border-cyber-border rounded px-2 py-1 text-xs font-mono text-cyber-text focus:outline-none focus:border-cyber-cyan"
          placeholder="bloxroute,flashbots,mevblocker"
        />
        <p className="mt-1 text-xs text-cyber-muted">
          Comma-separated provider names
        </p>
        <button
          onClick={handleRelayOrderUpdate}
          className="mt-2 w-full px-3 py-1 bg-cyber-cyan/20 text-cyber-cyan border border-cyber-cyan/30 rounded text-xs font-medium hover:bg-cyber-cyan/30 transition-colors"
        >
          APPLY RELAY ORDER
        </button>
      </div>

      {/* Emergency Rescue */}
      <div className="p-3 bg-cyber-red/5 rounded border border-cyber-red/20">
        <div className="flex items-center gap-2 mb-2">
          <AlertTriangle className="w-3 h-3 text-cyber-red" />
          <span className="text-xs font-medium text-cyber-red">EMERGENCY RESCUE</span>
        </div>
        <p className="text-xs text-cyber-muted mb-2">
          Withdraw all tokens from contract
        </p>
        {!showRescueConfirm ? (
          <button
            onClick={() => setShowRescueConfirm(true)}
            className="w-full px-3 py-1 bg-cyber-red/20 text-cyber-red border border-cyber-red/30 rounded text-xs font-medium hover:bg-cyber-red/30 transition-colors"
          >
            INITIATE RESCUE
          </button>
        ) : (
          <div className="space-y-2">
            <p className="text-xs text-cyber-red">Are you sure? Type CONFIRM:</p>
            <input
              type="text"
              className="w-full bg-cyber-black border border-cyber-red/50 rounded px-2 py-1 text-xs font-mono text-cyber-red focus:outline-none"
              placeholder="CONFIRM"
            />
            <div className="flex gap-2">
              <button
                onClick={() => setShowRescueConfirm(false)}
                className="flex-1 px-3 py-1 bg-cyber-border text-cyber-muted rounded text-xs font-medium hover:bg-cyber-muted/20 transition-colors"
              >
                CANCEL
              </button>
              <button
                onClick={() => console.log('Executing rescue...')}
                className="flex-1 px-3 py-1 bg-cyber-red text-white rounded text-xs font-medium hover:bg-cyber-red/80 transition-colors"
              >
                EXECUTE
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
