import { ReactNode } from 'react';
import { clsx } from 'clsx';

interface MetricCardProps {
  title: string;
  icon: ReactNode;
  accent: 'green' | 'cyan' | 'orange' | 'red';
  children: ReactNode;
}

export function MetricCard({ title, icon, accent, children }: MetricCardProps) {
  const accentColors = {
    green: 'border-cyber-green/30 hover:border-cyber-green/50',
    cyan: 'border-cyber-cyan/30 hover:border-cyber-cyan/50',
    orange: 'border-cyber-orange/30 hover:border-cyber-orange/50',
    red: 'border-cyber-red/30 hover:border-cyber-red/50',
  };

  const iconColors = {
    green: 'text-cyber-green',
    cyan: 'text-cyber-cyan',
    orange: 'text-cyber-orange',
    red: 'text-cyber-red',
  };

  return (
    <div 
      className={clsx(
        'bg-cyber-panel border rounded-lg p-4 transition-colors',
        accentColors[accent]
      )}
    >
      <div className="flex items-center gap-2 mb-2">
        <span className={iconColors[accent]}>{icon}</span>
        <span className="text-xs font-semibold text-cyber-muted tracking-wider">
          {title}
        </span>
      </div>
      {children}
    </div>
  );
}
