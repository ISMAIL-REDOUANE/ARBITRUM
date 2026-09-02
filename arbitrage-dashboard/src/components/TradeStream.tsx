import { useState } from 'react';
import type { TradeExecution } from '../types';
import { formatTimestamp, getStatusBadgeColor } from '../utils/mockData';
import { ExternalLink, Copy, ChevronDown, ChevronUp } from 'lucide-react';

export function TradeStream({ trades }: { trades: TradeExecution[] }) {
  const [isExpanded, setIsExpanded] = useState(true);
  const [copiedHash, setCopiedHash] = useState<string | null>(null);

  const copyTxHash = (hash: string) => {
    navigator.clipboard.writeText(hash);
    setCopiedHash(hash);
    setTimeout(() => setCopiedHash(null), 2000);
  };

  const getEtherscanUrl = (hash: string) => 
    `https://arbiscan.io/tx/${hash}`;

  return (
    <div className="bg-cyber-panel border border-cyber-border rounded-lg">
      <div 
        className="flex items-center justify-between p-4 cursor-pointer border-b border-cyber-border"
        onClick={() => setIsExpanded(!isExpanded)}
      >
        <h2 className="text-sm font-semibold text-cyber-text tracking-wider">
          ARBITRAGE EXECUTION STREAM
        </h2>
        <div className="flex items-center gap-4">
          <span className="text-xs text-cyber-muted">
            {trades.length} trades
          </span>
          {isExpanded ? (
            <ChevronUp className="w-4 h-4 text-cyber-muted" />
          ) : (
            <ChevronDown className="w-4 h-4 text-cyber-muted" />
          )}
        </div>
      </div>

      {isExpanded && (
        <div className="max-h-96 overflow-y-auto">
          <table className="w-full text-xs">
            <thead className="bg-cyber-dark sticky top-0">
              <tr className="text-cyber-muted">
                <th className="px-3 py-2 text-left font-medium">TIMESTAMP</th>
                <th className="px-3 py-2 text-left font-medium">ROUTE</th>
                <th className="px-3 py-2 text-right font-medium">BORROW</th>
                <th className="px-3 py-2 text-right font-medium">EST PROFIT</th>
                <th className="px-3 py-2 text-right font-medium">NET PROFIT</th>
                <th className="px-3 py-2 text-right font-medium">GAS</th>
                <th className="px-3 py-2 text-center font-medium">RELAY</th>
                <th className="px-3 py-2 text-center font-medium">STATUS</th>
                <th className="px-3 py-2 text-left font-medium">TX HASH</th>
              </tr>
            </thead>
            <tbody>
              {trades.map((trade) => (
                <tr 
                  key={trade.id}
                  className="border-t border-cyber-border/50 hover:bg-cyber-dark/50 transition-colors"
                >
                  <td className="px-3 py-2 text-cyber-cyan font-mono">
                    {formatTimestamp(trade.timestamp).split('.')[1]}
                  </td>
                  <td className="px-3 py-2 text-cyber-text">
                    <span className="text-xs">{trade.route}</span>
                  </td>
                  <td className="px-3 py-2 text-right text-cyber-muted">
                    ${trade.borrowAmount} {trade.borrowToken}
                  </td>
                  <td className="px-3 py-2 text-right text-cyber-orange">
                    ${trade.estimatedProfitUsd.toFixed(2)}
                  </td>
                  <td className={`px-3 py-2 text-right font-medium ${
                    trade.actualProfitEth > 0 ? 'text-cyber-green' : 'text-cyber-red'
                  }`}>
                    {trade.actualProfitEth > 0 
                      ? `+${trade.actualProfitEth.toFixed(6)} ETH`
                      : `${trade.actualProfitEth.toFixed(6)} ETH`
                    }
                  </td>
                  <td className="px-3 py-2 text-right text-cyber-muted">
                    {trade.gasUsed.toLocaleString()} @ {trade.gasPriceGwei.toFixed(1)} gwei
                  </td>
                  <td className="px-3 py-2 text-center">
                    <span className={`text-xs font-medium ${
                      trade.relay === 'BloxRoute' ? 'text-cyber-cyan' :
                      trade.relay === 'Flashbots' ? 'text-cyber-green' :
                      trade.relay === 'MEV-Blocker' ? 'text-cyber-orange' :
                      'text-cyber-red'
                    }`}>
                      {trade.relay}
                    </span>
                  </td>
                  <td className="px-3 py-2 text-center">
                    <span className={`px-2 py-0.5 text-xs rounded border font-medium ${getStatusBadgeColor(trade.status)}`}>
                      {trade.status}
                    </span>
                  </td>
                  <td className="px-3 py-2">
                    <div className="flex items-center gap-2">
                      <span className="text-cyber-muted font-mono text-xs">
                        {trade.txHash.slice(0, 10)}...
                      </span>
                      <button 
                        onClick={(e) => {
                          e.stopPropagation();
                          copyTxHash(trade.txHash);
                        }}
                        className="p-1 hover:bg-cyber-border rounded transition-colors"
                        title="Copy hash"
                      >
                        <Copy className={`w-3 h-3 ${copiedHash === trade.txHash ? 'text-cyber-green' : 'text-cyber-muted'}`} />
                      </button>
                      <a 
                        href={getEtherscanUrl(trade.txHash)}
                        target="_blank"
                        rel="noopener noreferrer"
                        onClick={(e) => e.stopPropagation()}
                        className="p-1 hover:bg-cyber-border rounded transition-colors"
                        title="View on Arbiscan"
                      >
                        <ExternalLink className="w-3 h-3 text-cyber-muted hover:text-cyber-cyan" />
                      </a>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
