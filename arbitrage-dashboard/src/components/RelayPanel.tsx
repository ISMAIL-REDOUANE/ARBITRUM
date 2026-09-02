import type { RelayStats, RelayProvider } from '../types';
import { Radio, CheckCircle, XCircle, Activity } from 'lucide-react';
import { clsx } from 'clsx';

interface RelayPanelProps {
  relays: RelayStats[];
}

const RELAY_COLORS: Record<RelayProvider, string> = {
  BloxRoute: 'text-cyber-cyan',
  Flashbots: 'text-cyber-green',
  'MEV-Blocker': 'text-cyber-orange',
  Public: 'text-cyber-red',
};

export function RelayPanel({ relays }: RelayPanelProps) {
  const sortedRelays = [...relays].sort((a, b) => a.priority - b.priority);

  return (
    <div className="bg-cyber-panel border border-cyber-border rounded-lg p-4">
      <div className="flex items-center gap-2 mb-4">
        <Radio className="w-4 h-4 text-cyber-green" />
        <h2 className="text-sm font-semibold text-cyber-text tracking-wider">
          RELAY PRIORITY
        </h2>
      </div>

      <div className="space-y-2">
        {sortedRelays.map((relay, idx) => (
          <div 
            key={relay.provider}
            className={clsx(
              'p-3 rounded border transition-colors',
              relay.isActive 
                ? 'bg-cyber-green/5 border-cyber-green/30' 
                : 'bg-cyber-dark/50 border-cyber-border'
            )}
          >
            <div className="flex items-center justify-between mb-2">
              <div className="flex items-center gap-2">
                <span 
                  className={clsx(
                    'text-sm font-bold',
                    RELAY_COLORS[relay.provider]
                  )}
                >
                  {relay.provider}
                </span>
                {relay.isActive && (
                  <span className="px-2 py-0.5 text-xs bg-cyber-green/20 text-cyber-green rounded">
                    ACTIVE
                  </span>
                )}
                <span className="text-xs text-cyber-muted">
                  #{idx + 1}
                </span>
              </div>
              <div className="flex items-center gap-1">
                {relay.successRate > 95 ? (
                  <CheckCircle className="w-3 h-3 text-cyber-green" />
                ) : (
                  <XCircle className="w-3 h-3 text-cyber-red" />
                )}
              </div>
            </div>

            <div className="grid grid-cols-3 gap-2 text-xs">
              <div>
                <span className="text-cyber-muted">Latency</span>
                <div className={clsx(
                  'font-mono font-medium',
                  relay.latencyMs < 50 ? 'text-cyber-green' :
                  relay.latencyMs < 100 ? 'text-cyber-orange' :
                  'text-cyber-red'
                )}>
                  {relay.latencyMs.toFixed(1)}ms
                </div>
              </div>
              <div>
                <span className="text-cyber-muted">Success</span>
                <div className={clsx(
                  'font-mono font-medium',
                  relay.successRate > 97 ? 'text-cyber-green' :
                  relay.successRate > 90 ? 'text-cyber-orange' :
                  'text-cyber-red'
                )}>
                  {relay.successRate.toFixed(1)}%
                </div>
              </div>
              <div>
                <span className="text-cyber-muted">Requests</span>
                <div className="font-mono text-cyber-text">
                  {(relay.totalRequests / 1000).toFixed(0)}K
                </div>
              </div>
            </div>

            {/* Latency bar */}
            <div className="mt-2 h-1 bg-cyber-dark rounded overflow-hidden">
              <div 
                className={clsx(
                  'h-full transition-all duration-300',
                  relay.latencyMs < 50 ? 'bg-cyber-green' :
                  relay.latencyMs < 100 ? 'bg-cyber-orange' :
                  'bg-cyber-red'
                )}
                style={{ 
                  width: `${Math.min(100, (relay.latencyMs / 150) * 100)}%` 
                }}
              />
            </div>
          </div>
        ))}
      </div>

      <div className="mt-4 p-3 bg-cyber-dark/50 rounded border border-cyber-border">
        <div className="flex items-center gap-2 text-xs text-cyber-muted">
          <Activity className="w-3 h-3" />
          <span>Round-trip latency: {(relays.find(r => r.isActive)?.latencyMs || 0).toFixed(1)}ms</span>
        </div>
      </div>
    </div>
  );
}
