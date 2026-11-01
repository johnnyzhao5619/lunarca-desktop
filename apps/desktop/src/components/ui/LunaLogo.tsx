import { type SVGProps } from "react";

export interface LunaLogoProps extends SVGProps<SVGSVGElement> {
  size?: number | string;
}

export function LunaLogo({ size = 48, className, ...props }: LunaLogoProps) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 128 128"
      className={className}
      aria-hidden="true"
      {...props}
    >
      <circle cx="64" cy="64" r="44" fill="#45454c" />
      <path d="M64 20a44 44 0 0 0 0 88c-15-12-20-27-20-44s5-32 20-44Z" fill="#f3d998" />
      <circle cx="39" cy="50" r="3" fill="#c99b58" />
      <circle cx="38" cy="76" r="3" fill="#c99b58" />
      <circle cx="79" cy="43" r="5" fill="#5d5d66" />
      <circle cx="89" cy="64" r="4" fill="#5d5d66" />
      <circle cx="77" cy="82" r="6" fill="#5d5d66" />
    </svg>
  );
}
