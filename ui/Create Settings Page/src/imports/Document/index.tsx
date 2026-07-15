import svgPaths from "./svg-mfnfu8zeoz";

function SessionListPanel() {
  return (
    <div className="relative shrink-0" data-name="SessionListPanel">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[18px] not-italic relative shrink-0 text-[#e5e5ea] text-[12px] whitespace-nowrap">Sessions</p>
      </div>
    </div>
  );
}

function SessionListPanel1() {
  return (
    <div className="bg-[rgba(255,255,255,0.06)] relative rounded-[4px] shrink-0" data-name="SessionListPanel">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[6px] py-[2px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">10</p>
      </div>
    </div>
  );
}

function Container1() {
  return (
    <div className="flex-[320_0_0] min-w-px relative" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <SessionListPanel />
        <SessionListPanel1 />
      </div>
    </div>
  );
}

function PanelHeader() {
  return (
    <div className="h-[48px] relative shrink-0 w-full" data-name="PanelHeader">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center pb-px pl-[12px] pr-[8px] relative size-full">
          <Container1 />
        </div>
      </div>
    </div>
  );
}

function Icon() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_955)" id="Icon">
          <path d={svgPaths.p2c2cc780} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M9.625 9.625L7.65417 7.65417" id="Vector_2" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_955">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function TextInput() {
  return (
    <div className="flex-[284.75_0_0] h-[16.5px] min-w-px relative" data-name="Text Input">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start justify-center overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[normal] not-italic relative shrink-0 text-[11px] text-[rgba(229,229,234,0.5)] w-full">Search sessions…</p>
      </div>
    </div>
  );
}

function SearchInput() {
  return (
    <div className="bg-[rgba(255,255,255,0.06)] flex-[320.5_0_0] h-[28px] min-w-px relative rounded-[8.125px]" data-name="SearchInput">
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[9.125px] py-px relative size-full">
          <Icon />
          <TextInput />
        </div>
      </div>
    </div>
  );
}

function FilterRow() {
  return (
    <div className="bg-[rgba(28,28,30,0.8)] h-[40px] relative shrink-0 w-full" data-name="FilterRow">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center pb-px px-[9.75px] relative size-full">
          <SearchInput />
        </div>
      </div>
    </div>
  );
}

function Text() {
  return (
    <div className="col-1 justify-self-stretch relative row-1 self-stretch shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">Session</p>
      </div>
    </div>
  );
}

function Text1() {
  return (
    <div className="col-2 justify-self-stretch relative row-1 self-stretch shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">T</p>
      </div>
    </div>
  );
}

function Text2() {
  return (
    <div className="col-3 justify-self-stretch relative row-1 self-stretch shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">Cost</p>
      </div>
    </div>
  );
}

function Text3() {
  return (
    <div className="col-4 justify-self-stretch relative row-1 self-stretch shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">Dur</p>
      </div>
    </div>
  );
}

function Text4() {
  return (
    <div className="col-5 justify-self-stretch relative row-1 self-stretch shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">Account</p>
      </div>
    </div>
  );
}

function SessionListPanel2() {
  return (
    <div className="h-[24px] relative shrink-0 w-full" data-name="SessionListPanel">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid grid grid-cols-[_____120px_26px_50px_44px_72px] grid-rows-[_23px] pb-px pl-[20px] pr-[8px] relative size-full">
        <Text />
        <Text1 />
        <Text2 />
        <Text3 />
        <Text4 />
      </div>
    </div>
  );
}

function Button() {
  return (
    <div className="bg-[rgba(255,255,255,0.1)] relative rounded-[6px] shrink-0" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center justify-center px-[8px] py-[2px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15px] not-italic relative shrink-0 text-[#e5e5ea] text-[10px] text-center whitespace-nowrap">All</p>
      </div>
    </div>
  );
}

function Button1() {
  return (
    <div className="relative rounded-[6px] shrink-0" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center justify-center px-[8px] py-[2px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-center whitespace-nowrap">Running</p>
      </div>
    </div>
  );
}

function Button2() {
  return (
    <div className="relative rounded-[6px] shrink-0" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center justify-center px-[8px] py-[2px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-center whitespace-nowrap">Error</p>
      </div>
    </div>
  );
}

function Button3() {
  return (
    <div className="relative rounded-[6px] shrink-0" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center justify-center px-[8px] py-[2px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-center whitespace-nowrap">Done</p>
      </div>
    </div>
  );
}

function SessionListPanel3() {
  return (
    <div className="h-[32px] relative shrink-0 w-full" data-name="SessionListPanel">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center pb-px px-[9.75px] relative size-full">
          <Button />
          <Button1 />
          <Button2 />
          <Button3 />
        </div>
      </div>
    </div>
  );
}

function Icon1() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d={svgPaths.p26a1c040} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Button4() {
  return (
    <div className="relative shrink-0 w-[16px]" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon1 />
      </div>
    </div>
  );
}

function StatusDot() {
  return <div className="bg-[#30d158] relative rounded-[16777200px] shrink-0 size-[4.875px]" data-name="StatusDot" />;
}

function Icon2() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g id="Icon">
          <path d={svgPaths.p2b4b6d80} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d="M4.5 7.125H7.5" id="Vector_2" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
        </g>
      </svg>
    </div>
  );
}

function HarnessIcon() {
  return (
    <div className="bg-[rgba(10,132,255,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="HarnessIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon2 />
      </div>
    </div>
  );
}

function ProviderBadge() {
  return (
    <div className="bg-[rgba(255,107,0,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="ProviderBadge">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[13.5px] not-italic relative shrink-0 text-[#ff9040] text-[9px] whitespace-nowrap">A</p>
      </div>
    </div>
  );
}

function ModelBadge() {
  return (
    <div className="bg-[rgba(191,90,242,0.12)] relative rounded-[5px] shrink-0" data-name="ModelBadge">
      <div aria-hidden className="absolute border border-[rgba(191,90,242,0.25)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[7px] py-[3px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#bf5af2] text-[9.5px] whitespace-nowrap">opus 4.8</p>
      </div>
    </div>
  );
}

function Container2() {
  return (
    <div className="col-1 justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center relative size-full">
        <Button4 />
        <StatusDot />
        <HarnessIcon />
        <ProviderBadge />
        <ModelBadge />
      </div>
    </div>
  );
}

function Container3() {
  return (
    <div className="col-2 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">7</p>
      </div>
    </div>
  );
}

function Container4() {
  return (
    <div className="col-3 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">$0.84</p>
      </div>
    </div>
  );
}

function Container5() {
  return (
    <div className="col-4 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">42.1s</p>
      </div>
    </div>
  );
}

function Container6() {
  return (
    <div className="col-5 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] text-right whitespace-nowrap">prod-api</p>
      </div>
    </div>
  );
}

function SessionRow() {
  return (
    <div className="bg-[rgba(10,132,255,0.07)] h-[30px] relative shrink-0 w-full" data-name="SessionRow">
      <div aria-hidden className="absolute border-[#0a84ff] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid grid grid-cols-[_____134px_26px_50px_44px_72px] grid-rows-[_30px] pl-[4px] pr-[10px] relative size-full">
        <Container2 />
        <Container3 />
        <Container4 />
        <Container5 />
        <Container6 />
      </div>
    </div>
  );
}

function Container9() {
  return <div className="bg-[#3a3a3c] h-[12px] relative shrink-0 w-px" data-name="Container" />;
}

function ContainerMargin() {
  return (
    <div className="relative shrink-0" data-name="Container:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-start pr-[3px] relative size-full">
        <Container9 />
      </div>
    </div>
  );
}

function Container10() {
  return <div className="bg-[#3a3a3c] h-px relative shrink-0 w-[8px]" data-name="Container" />;
}

function Container8() {
  return (
    <div className="relative shrink-0 w-[24px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center pl-[8px] relative size-full">
        <ContainerMargin />
        <Container10 />
      </div>
    </div>
  );
}

function StatusDot1() {
  return <div className="bg-[#30d158] relative rounded-[16777200px] shrink-0 size-[4.875px]" data-name="StatusDot" />;
}

function Icon3() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g id="Icon">
          <path d={svgPaths.p2b4b6d80} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d="M4.5 7.125H7.5" id="Vector_2" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
        </g>
      </svg>
    </div>
  );
}

function HarnessIcon1() {
  return (
    <div className="bg-[rgba(10,132,255,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="HarnessIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon3 />
      </div>
    </div>
  );
}

function ProviderBadge1() {
  return (
    <div className="bg-[rgba(255,107,0,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="ProviderBadge">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[13.5px] not-italic relative shrink-0 text-[#ff9040] text-[9px] whitespace-nowrap">A</p>
      </div>
    </div>
  );
}

function ModelBadge1() {
  return (
    <div className="bg-[rgba(10,132,255,0.12)] relative rounded-[5px] shrink-0" data-name="ModelBadge">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.25)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[7px] py-[3px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#409cff] text-[9.5px] whitespace-nowrap">sonnet 4.6</p>
      </div>
    </div>
  );
}

function Container7() {
  return (
    <div className="col-1 justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center relative size-full">
        <Container8 />
        <StatusDot1 />
        <HarnessIcon1 />
        <ProviderBadge1 />
        <ModelBadge1 />
      </div>
    </div>
  );
}

function Container11() {
  return (
    <div className="col-2 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">6</p>
      </div>
    </div>
  );
}

function Container12() {
  return (
    <div className="col-3 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">$0.18</p>
      </div>
    </div>
  );
}

function Container13() {
  return (
    <div className="col-4 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">8.4s</p>
      </div>
    </div>
  );
}

function Container14() {
  return (
    <div className="col-5 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] text-right whitespace-nowrap">prod-api</p>
      </div>
    </div>
  );
}

function SessionRow1() {
  return (
    <div className="h-[30px] relative shrink-0 w-full" data-name="SessionRow">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid grid grid-cols-[_____138px_26px_50px_44px_72px] grid-rows-[_30px] pr-[10px] relative size-full">
        <Container7 />
        <Container11 />
        <Container12 />
        <Container13 />
        <Container14 />
      </div>
    </div>
  );
}

function Container16() {
  return <div className="relative shrink-0 size-0" data-name="Container" />;
}

function StatusDot2() {
  return <div className="bg-[#ff453a] relative rounded-[16777200px] shrink-0 size-[4.875px]" data-name="StatusDot" />;
}

function Icon4() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g clipPath="url(#clip0_1_931)" id="Icon">
          <path d={svgPaths.p352d0d00} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d={svgPaths.p37fd1d00} id="Vector_2" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d="M0.75 4.5H8.25" id="Vector_3" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
        </g>
        <defs>
          <clipPath id="clip0_1_931">
            <rect fill="white" height="9" width="9" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function HarnessIcon2() {
  return (
    <div className="bg-[rgba(191,90,242,0.15)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="HarnessIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon4 />
      </div>
    </div>
  );
}

function ProviderBadge2() {
  return (
    <div className="bg-[rgba(255,107,0,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="ProviderBadge">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[13.5px] not-italic relative shrink-0 text-[#ff9040] text-[9px] whitespace-nowrap">A</p>
      </div>
    </div>
  );
}

function ModelBadge2() {
  return (
    <div className="bg-[rgba(10,132,255,0.12)] relative rounded-[5px] shrink-0" data-name="ModelBadge">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.25)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[7px] py-[3px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#409cff] text-[9.5px] whitespace-nowrap">sonnet 4.6</p>
      </div>
    </div>
  );
}

function Container15() {
  return (
    <div className="col-1 justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center relative size-full">
        <Container16 />
        <StatusDot2 />
        <HarnessIcon2 />
        <ProviderBadge2 />
        <ModelBadge2 />
      </div>
    </div>
  );
}

function Container17() {
  return (
    <div className="col-2 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">3</p>
      </div>
    </div>
  );
}

function Container18() {
  return (
    <div className="col-3 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">$0.12</p>
      </div>
    </div>
  );
}

function Container19() {
  return (
    <div className="col-4 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">8.9s</p>
      </div>
    </div>
  );
}

function Container20() {
  return (
    <div className="col-5 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] text-right whitespace-nowrap">staging</p>
      </div>
    </div>
  );
}

function SessionRow2() {
  return (
    <div className="h-[30px] relative shrink-0 w-full" data-name="SessionRow">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid grid grid-cols-[_____134px_26px_50px_44px_72px] grid-rows-[_30px] pl-[4px] pr-[10px] relative size-full">
        <Container15 />
        <Container17 />
        <Container18 />
        <Container19 />
        <Container20 />
      </div>
    </div>
  );
}

function Container22() {
  return <div className="h-0 relative shrink-0 w-[5.133px]" data-name="Container" />;
}

function StatusDot3() {
  return <div className="bg-[#ffd60a] relative rounded-[16777200px] shrink-0 size-[4.875px]" data-name="StatusDot" />;
}

function Icon5() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g id="Icon">
          <path d={svgPaths.p1cecf7c0} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d={svgPaths.p1ff921c0} id="Vector_2" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d="M3 4.5H6" id="Vector_3" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
        </g>
      </svg>
    </div>
  );
}

function HarnessIcon3() {
  return (
    <div className="bg-[rgba(90,200,250,0.15)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="HarnessIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon5 />
      </div>
    </div>
  );
}

function ProviderBadge3() {
  return (
    <div className="bg-[rgba(16,185,129,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="ProviderBadge">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[13.5px] not-italic relative shrink-0 text-[#10b981] text-[9px] whitespace-nowrap">O</p>
      </div>
    </div>
  );
}

function Text5() {
  return (
    <div className="h-[16.5px] relative shrink-0 w-[17.297px]" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#aeaeb2] text-[11px] tracking-[0.11px] whitespace-nowrap">D5E3F1A2</p>
      </div>
    </div>
  );
}

function ModelBadge3() {
  return (
    <div className="bg-[rgba(90,200,250,0.1)] relative rounded-[5px] shrink-0" data-name="ModelBadge">
      <div aria-hidden className="absolute border border-[rgba(90,200,250,0.22)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[7px] py-[3px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#5ac8fa] text-[9.5px] whitespace-nowrap">gpt-4o</p>
      </div>
    </div>
  );
}

function Container21() {
  return (
    <div className="col-1 justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center relative size-full">
        <Container22 />
        <StatusDot3 />
        <HarnessIcon3 />
        <ProviderBadge3 />
        <Text5 />
        <ModelBadge3 />
      </div>
    </div>
  );
}

function Container23() {
  return (
    <div className="col-2 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">2</p>
      </div>
    </div>
  );
}

function Container24() {
  return (
    <div className="col-3 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">$0.03</p>
      </div>
    </div>
  );
}

function Container25() {
  return (
    <div className="col-4 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">12.3s</p>
      </div>
    </div>
  );
}

function Container26() {
  return (
    <div className="col-5 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] text-right whitespace-nowrap">dev-oai</p>
      </div>
    </div>
  );
}

function SessionRow3() {
  return (
    <div className="h-[30px] relative shrink-0 w-full" data-name="SessionRow">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid grid grid-cols-[_____134px_26px_50px_44px_72px] grid-rows-[_30px] pl-[4px] pr-[10px] relative size-full">
        <Container21 />
        <Container23 />
        <Container24 />
        <Container25 />
        <Container26 />
      </div>
    </div>
  );
}

function Icon6() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d={svgPaths.p26a1c040} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Button5() {
  return (
    <div className="relative shrink-0 w-[16px]" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon6 />
      </div>
    </div>
  );
}

function StatusDot4() {
  return <div className="bg-[#30d158] relative rounded-[16777200px] shrink-0 size-[4.875px]" data-name="StatusDot" />;
}

function Icon7() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g id="Icon">
          <path d={svgPaths.p2b4b6d80} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d="M4.5 7.125H7.5" id="Vector_2" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
        </g>
      </svg>
    </div>
  );
}

function HarnessIcon4() {
  return (
    <div className="bg-[rgba(10,132,255,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="HarnessIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon7 />
      </div>
    </div>
  );
}

function ProviderBadge4() {
  return (
    <div className="bg-[rgba(255,107,0,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="ProviderBadge">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[13.5px] not-italic relative shrink-0 text-[#ff9040] text-[9px] whitespace-nowrap">A</p>
      </div>
    </div>
  );
}

function ModelBadge4() {
  return (
    <div className="bg-[rgba(48,209,88,0.1)] relative rounded-[5px] shrink-0" data-name="ModelBadge">
      <div aria-hidden className="absolute border border-[rgba(48,209,88,0.22)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[7px] py-[3px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#30d158] text-[9.5px] whitespace-nowrap">haiku 4.5</p>
      </div>
    </div>
  );
}

function Container27() {
  return (
    <div className="col-1 justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center relative size-full">
        <Button5 />
        <StatusDot4 />
        <HarnessIcon4 />
        <ProviderBadge4 />
        <ModelBadge4 />
      </div>
    </div>
  );
}

function Container28() {
  return (
    <div className="col-2 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">12</p>
      </div>
    </div>
  );
}

function Container29() {
  return (
    <div className="col-3 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">$0.09</p>
      </div>
    </div>
  );
}

function Container30() {
  return (
    <div className="col-4 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">28.4s</p>
      </div>
    </div>
  );
}

function Container31() {
  return (
    <div className="col-5 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] text-right whitespace-nowrap">prod-api</p>
      </div>
    </div>
  );
}

function SessionRow4() {
  return (
    <div className="h-[30px] relative shrink-0 w-full" data-name="SessionRow">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid grid grid-cols-[_____134px_26px_50px_44px_72px] grid-rows-[_30px] pl-[4px] pr-[10px] relative size-full">
        <Container27 />
        <Container28 />
        <Container29 />
        <Container30 />
        <Container31 />
      </div>
    </div>
  );
}

function Container34() {
  return <div className="bg-[#3a3a3c] h-[12px] relative shrink-0 w-px" data-name="Container" />;
}

function ContainerMargin1() {
  return (
    <div className="relative shrink-0" data-name="Container:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-start pr-[3px] relative size-full">
        <Container34 />
      </div>
    </div>
  );
}

function Container35() {
  return <div className="bg-[#3a3a3c] h-px relative shrink-0 w-[8px]" data-name="Container" />;
}

function Container33() {
  return (
    <div className="relative shrink-0 w-[24px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center pl-[8px] relative size-full">
        <ContainerMargin1 />
        <Container35 />
      </div>
    </div>
  );
}

function StatusDot5() {
  return <div className="bg-[#30d158] relative rounded-[16777200px] shrink-0 size-[4.875px]" data-name="StatusDot" />;
}

function Icon8() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g id="Icon">
          <path d={svgPaths.p2b4b6d80} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d="M4.5 7.125H7.5" id="Vector_2" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
        </g>
      </svg>
    </div>
  );
}

function HarnessIcon5() {
  return (
    <div className="bg-[rgba(10,132,255,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="HarnessIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon8 />
      </div>
    </div>
  );
}

function ProviderBadge5() {
  return (
    <div className="bg-[rgba(255,107,0,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="ProviderBadge">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[13.5px] not-italic relative shrink-0 text-[#ff9040] text-[9px] whitespace-nowrap">A</p>
      </div>
    </div>
  );
}

function ModelBadge5() {
  return (
    <div className="bg-[rgba(48,209,88,0.1)] relative rounded-[5px] shrink-0" data-name="ModelBadge">
      <div aria-hidden className="absolute border border-[rgba(48,209,88,0.22)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[7px] py-[3px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#30d158] text-[9.5px] whitespace-nowrap">haiku 4.5</p>
      </div>
    </div>
  );
}

function Container32() {
  return (
    <div className="col-1 justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center relative size-full">
        <Container33 />
        <StatusDot5 />
        <HarnessIcon5 />
        <ProviderBadge5 />
        <ModelBadge5 />
      </div>
    </div>
  );
}

function Container36() {
  return (
    <div className="col-2 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">4</p>
      </div>
    </div>
  );
}

function Container37() {
  return (
    <div className="col-3 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">$0.02</p>
      </div>
    </div>
  );
}

function Container38() {
  return (
    <div className="col-4 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">5.1s</p>
      </div>
    </div>
  );
}

function Container39() {
  return (
    <div className="col-5 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] text-right whitespace-nowrap">prod-api</p>
      </div>
    </div>
  );
}

function SessionRow5() {
  return (
    <div className="h-[30px] relative shrink-0 w-full" data-name="SessionRow">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid grid grid-cols-[_____138px_26px_50px_44px_72px] grid-rows-[_30px] pr-[10px] relative size-full">
        <Container32 />
        <Container36 />
        <Container37 />
        <Container38 />
        <Container39 />
      </div>
    </div>
  );
}

function Container41() {
  return <div className="h-0 relative shrink-0 w-[2.516px]" data-name="Container" />;
}

function StatusDot6() {
  return <div className="bg-[#30d158] relative rounded-[16777200px] shrink-0 size-[4.875px]" data-name="StatusDot" />;
}

function Icon9() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g clipPath="url(#clip0_1_946)" id="Icon">
          <path d={svgPaths.pc83c500} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d={svgPaths.p1761900} id="Vector_2" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
        </g>
        <defs>
          <clipPath id="clip0_1_946">
            <rect fill="white" height="9" width="9" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function HarnessIcon6() {
  return (
    <div className="bg-[rgba(255,255,255,0.08)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="HarnessIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon9 />
      </div>
    </div>
  );
}

function ProviderBadge6() {
  return (
    <div className="bg-[rgba(255,107,0,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="ProviderBadge">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[13.5px] not-italic relative shrink-0 text-[#ff9040] text-[9px] whitespace-nowrap">A</p>
      </div>
    </div>
  );
}

function Text6() {
  return (
    <div className="h-[16.5px] relative shrink-0 w-[8.477px]" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#aeaeb2] text-[11px] tracking-[0.11px] whitespace-nowrap">A1B2C3D4</p>
      </div>
    </div>
  );
}

function ModelBadge6() {
  return (
    <div className="bg-[rgba(191,90,242,0.12)] relative rounded-[5px] shrink-0" data-name="ModelBadge">
      <div aria-hidden className="absolute border border-[rgba(191,90,242,0.25)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[7px] py-[3px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#bf5af2] text-[9.5px] whitespace-nowrap">opus 4.8</p>
      </div>
    </div>
  );
}

function Container40() {
  return (
    <div className="col-1 justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center relative size-full">
        <Container41 />
        <StatusDot6 />
        <HarnessIcon6 />
        <ProviderBadge6 />
        <Text6 />
        <ModelBadge6 />
      </div>
    </div>
  );
}

function Container42() {
  return (
    <div className="col-2 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">5</p>
      </div>
    </div>
  );
}

function Container43() {
  return (
    <div className="col-3 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">$0.61</p>
      </div>
    </div>
  );
}

function Container44() {
  return (
    <div className="col-4 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">31.2s</p>
      </div>
    </div>
  );
}

function Container45() {
  return (
    <div className="col-5 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] text-right whitespace-nowrap">prod-api</p>
      </div>
    </div>
  );
}

function SessionRow6() {
  return (
    <div className="h-[30px] relative shrink-0 w-full" data-name="SessionRow">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid grid grid-cols-[_____134px_26px_50px_44px_72px] grid-rows-[_30px] pl-[4px] pr-[10px] relative size-full">
        <Container40 />
        <Container42 />
        <Container43 />
        <Container44 />
        <Container45 />
      </div>
    </div>
  );
}

function Container47() {
  return <div className="relative shrink-0 size-0" data-name="Container" />;
}

function StatusDot7() {
  return <div className="bg-[#ff453a] relative rounded-[16777200px] shrink-0 size-[4.875px]" data-name="StatusDot" />;
}

function Icon10() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g clipPath="url(#clip0_1_931)" id="Icon">
          <path d={svgPaths.p352d0d00} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d={svgPaths.p37fd1d00} id="Vector_2" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d="M0.75 4.5H8.25" id="Vector_3" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
        </g>
        <defs>
          <clipPath id="clip0_1_931">
            <rect fill="white" height="9" width="9" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function HarnessIcon7() {
  return (
    <div className="bg-[rgba(191,90,242,0.15)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="HarnessIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon10 />
      </div>
    </div>
  );
}

function ProviderBadge7() {
  return (
    <div className="bg-[rgba(255,107,0,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="ProviderBadge">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[13.5px] not-italic relative shrink-0 text-[#ff9040] text-[9px] whitespace-nowrap">A</p>
      </div>
    </div>
  );
}

function ModelBadge7() {
  return (
    <div className="bg-[rgba(10,132,255,0.12)] relative rounded-[5px] shrink-0" data-name="ModelBadge">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.25)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[7px] py-[3px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#409cff] text-[9.5px] whitespace-nowrap">sonnet 4.6</p>
      </div>
    </div>
  );
}

function Container46() {
  return (
    <div className="col-1 justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center relative size-full">
        <Container47 />
        <StatusDot7 />
        <HarnessIcon7 />
        <ProviderBadge7 />
        <ModelBadge7 />
      </div>
    </div>
  );
}

function Container48() {
  return (
    <div className="col-2 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">1</p>
      </div>
    </div>
  );
}

function Container49() {
  return (
    <div className="col-3 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">—</p>
      </div>
    </div>
  );
}

function Container50() {
  return (
    <div className="col-4 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">1.2s</p>
      </div>
    </div>
  );
}

function Container51() {
  return (
    <div className="col-5 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] text-right whitespace-nowrap">staging</p>
      </div>
    </div>
  );
}

function SessionRow7() {
  return (
    <div className="h-[30px] relative shrink-0 w-full" data-name="SessionRow">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid grid grid-cols-[_____134px_26px_50px_44px_72px] grid-rows-[_30px] pl-[4px] pr-[10px] relative size-full">
        <Container46 />
        <Container48 />
        <Container49 />
        <Container50 />
        <Container51 />
      </div>
    </div>
  );
}

function Container53() {
  return <div className="relative shrink-0 size-0" data-name="Container" />;
}

function StatusDot8() {
  return <div className="bg-[#30d158] relative rounded-[16777200px] shrink-0 size-[4.875px]" data-name="StatusDot" />;
}

function Icon11() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g id="Icon">
          <path d={svgPaths.p2b4b6d80} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d="M4.5 7.125H7.5" id="Vector_2" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
        </g>
      </svg>
    </div>
  );
}

function HarnessIcon8() {
  return (
    <div className="bg-[rgba(10,132,255,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="HarnessIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon11 />
      </div>
    </div>
  );
}

function ProviderBadge8() {
  return (
    <div className="bg-[rgba(255,107,0,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="ProviderBadge">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[13.5px] not-italic relative shrink-0 text-[#ff9040] text-[9px] whitespace-nowrap">A</p>
      </div>
    </div>
  );
}

function ModelBadge8() {
  return (
    <div className="bg-[rgba(10,132,255,0.12)] relative rounded-[5px] shrink-0" data-name="ModelBadge">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.25)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[7px] py-[3px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#409cff] text-[9.5px] whitespace-nowrap">sonnet 4.6</p>
      </div>
    </div>
  );
}

function Container52() {
  return (
    <div className="col-1 justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center relative size-full">
        <Container53 />
        <StatusDot8 />
        <HarnessIcon8 />
        <ProviderBadge8 />
        <ModelBadge8 />
      </div>
    </div>
  );
}

function Container54() {
  return (
    <div className="col-2 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">9</p>
      </div>
    </div>
  );
}

function Container55() {
  return (
    <div className="col-3 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">$0.27</p>
      </div>
    </div>
  );
}

function Container56() {
  return (
    <div className="col-4 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">19.8s</p>
      </div>
    </div>
  );
}

function Container57() {
  return (
    <div className="col-5 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] text-right whitespace-nowrap">prod-api</p>
      </div>
    </div>
  );
}

function SessionRow8() {
  return (
    <div className="h-[30px] relative shrink-0 w-full" data-name="SessionRow">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid grid grid-cols-[_____134px_26px_50px_44px_72px] grid-rows-[_30px] pl-[4px] pr-[10px] relative size-full">
        <Container52 />
        <Container54 />
        <Container55 />
        <Container56 />
        <Container57 />
      </div>
    </div>
  );
}

function Container59() {
  return <div className="h-0 relative shrink-0 w-[1.211px]" data-name="Container" />;
}

function StatusDot9() {
  return <div className="bg-[#636366] relative rounded-[16777200px] shrink-0 size-[4.875px]" data-name="StatusDot" />;
}

function Icon12() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g clipPath="url(#clip0_1_931)" id="Icon">
          <path d={svgPaths.p352d0d00} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d={svgPaths.p37fd1d00} id="Vector_2" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
          <path d="M0.75 4.5H8.25" id="Vector_3" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
        </g>
        <defs>
          <clipPath id="clip0_1_931">
            <rect fill="white" height="9" width="9" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function HarnessIcon9() {
  return (
    <div className="bg-[rgba(191,90,242,0.15)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="HarnessIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon12 />
      </div>
    </div>
  );
}

function ProviderBadge9() {
  return (
    <div className="bg-[rgba(255,107,0,0.18)] relative rounded-[3.25px] shrink-0 size-[17px]" data-name="ProviderBadge">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[13.5px] not-italic relative shrink-0 text-[#ff9040] text-[9px] whitespace-nowrap">A</p>
      </div>
    </div>
  );
}

function Text7() {
  return (
    <div className="h-[16.5px] relative shrink-0 w-[4.063px]" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#aeaeb2] text-[11px] tracking-[0.11px] whitespace-nowrap">D4E5F6A7</p>
      </div>
    </div>
  );
}

function ModelBadge9() {
  return (
    <div className="bg-[rgba(48,209,88,0.1)] relative rounded-[5px] shrink-0" data-name="ModelBadge">
      <div aria-hidden className="absolute border border-[rgba(48,209,88,0.22)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[7px] py-[3px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#30d158] text-[9.5px] whitespace-nowrap">haiku 4.5</p>
      </div>
    </div>
  );
}

function Container58() {
  return (
    <div className="col-1 justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center relative size-full">
        <Container59 />
        <StatusDot9 />
        <HarnessIcon9 />
        <ProviderBadge9 />
        <Text7 />
        <ModelBadge9 />
      </div>
    </div>
  );
}

function Container60() {
  return (
    <div className="col-2 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">0</p>
      </div>
    </div>
  );
}

function Container61() {
  return (
    <div className="col-3 h-[15.75px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">—</p>
      </div>
    </div>
  );
}

function Container62() {
  return (
    <div className="col-4 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">—</p>
      </div>
    </div>
  );
}

function Container63() {
  return (
    <div className="col-5 h-[15px] justify-self-stretch relative row-1 self-center shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] text-right whitespace-nowrap">dev</p>
      </div>
    </div>
  );
}

function SessionRow9() {
  return (
    <div className="h-[30px] relative shrink-0 w-full" data-name="SessionRow">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid grid grid-cols-[_____134px_26px_50px_44px_72px] grid-rows-[_30px] pl-[4px] pr-[10px] relative size-full">
        <Container58 />
        <Container60 />
        <Container61 />
        <Container62 />
        <Container63 />
      </div>
    </div>
  );
}

function SessionListPanel4() {
  return (
    <div className="flex-[650_0_0] min-h-px relative w-full" data-name="SessionListPanel">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <SessionRow />
        <SessionRow1 />
        <SessionRow2 />
        <SessionRow3 />
        <SessionRow4 />
        <SessionRow5 />
        <SessionRow6 />
        <SessionRow7 />
        <SessionRow8 />
        <SessionRow9 />
      </div>
    </div>
  );
}

function Text8() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">8 of 10 sessions</p>
      </div>
    </div>
  );
}

function SessionListPanel5() {
  return (
    <div className="h-[28px] relative shrink-0 w-full" data-name="SessionListPanel">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center pt-px px-[9.75px] relative size-full">
          <Text8 />
        </div>
      </div>
    </div>
  );
}

function Container() {
  return (
    <div className="h-full relative shrink-0 w-[340px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <PanelHeader />
        <FilterRow />
        <SessionListPanel2 />
        <SessionListPanel3 />
        <SessionListPanel4 />
        <SessionListPanel5 />
      </div>
    </div>
  );
}

function Container65() {
  return <div className="absolute bg-[rgba(255,255,255,0.07)] h-[822px] left-[2px] top-0 w-px" data-name="Container" />;
}

function Container64() {
  return (
    <div className="h-full relative shrink-0 w-[5px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid relative size-full">
        <Container65 />
      </div>
    </div>
  );
}

function Icon13() {
  return (
    <div className="relative shrink-0 size-[13px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 13 13">
        <g id="Icon">
          <path d={svgPaths.p1f555e80} id="Vector" stroke="var(--stroke-0, #0A84FF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.08333" />
        </g>
      </svg>
    </div>
  );
}

function TraceViewPanel() {
  return (
    <div className="bg-[rgba(10,132,255,0.15)] relative rounded-[8.125px] shrink-0 size-[30px]" data-name="TraceViewPanel">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.22)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center p-px relative size-full">
        <Icon13 />
      </div>
    </div>
  );
}

function Container68() {
  return (
    <div className="h-[16.5px] relative shrink-0 w-full" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Semi_Bold',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">A3F8B2D1</p>
      </div>
    </div>
  );
}

function Container69() {
  return (
    <div className="relative shrink-0 w-full" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">7 turns · 10 tools · 1 subagent</p>
      </div>
    </div>
  );
}

function TraceViewPanel1() {
  return (
    <div className="relative shrink-0 w-[136.906px]" data-name="TraceViewPanel">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Container68 />
        <Container69 />
      </div>
    </div>
  );
}

function ModelBadge10() {
  return (
    <div className="bg-[rgba(191,90,242,0.12)] relative rounded-[5px] shrink-0" data-name="ModelBadge">
      <div aria-hidden className="absolute border border-[rgba(191,90,242,0.25)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[7px] py-[3px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#bf5af2] text-[9.5px] whitespace-nowrap">opus 4.8</p>
      </div>
    </div>
  );
}

function Container67() {
  return (
    <div className="flex-[769.375_0_0] min-w-px relative" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <TraceViewPanel />
        <TraceViewPanel1 />
        <ModelBadge10 />
      </div>
    </div>
  );
}

function Icon14() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_911)" id="Icon">
          <path d={svgPaths.p130b2500} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p26b52e00} id="Vector_2" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_911">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function Text9() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-center whitespace-nowrap">Copy ID</p>
      </div>
    </div>
  );
}

function CopyButton() {
  return (
    <div className="relative rounded-[8.125px] shrink-0" data-name="CopyButton">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center px-[8px] py-[4px] relative size-full">
        <Icon14 />
        <Text9 />
      </div>
    </div>
  );
}

function Icon15() {
  return (
    <div className="relative shrink-0 size-[14px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 14 14">
        <g id="Icon">
          <path d={svgPaths.p173d3600} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.16667" />
          <path d={svgPaths.p3a793800} id="Vector_2" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.16667" />
          <path d={svgPaths.p37fa4800} id="Vector_3" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.16667" />
        </g>
      </svg>
    </div>
  );
}

function TraceViewPanel2() {
  return (
    <div className="relative rounded-[8.125px] shrink-0 size-[28px]" data-name="TraceViewPanel">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon15 />
      </div>
    </div>
  );
}

function Container70() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[1.625px] items-center relative size-full">
        <CopyButton />
        <TraceViewPanel2 />
      </div>
    </div>
  );
}

function PanelHeader1() {
  return (
    <div className="h-[48px] relative shrink-0" data-name="PanelHeader">
      <div aria-hidden className="absolute border-[#0a84ff] border-b border-l-2 border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center pb-px pl-[12px] pr-[8px] relative size-full">
        <Container67 />
        <Container70 />
      </div>
    </div>
  );
}

function Icon16() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_955)" id="Icon">
          <path d={svgPaths.p2c2cc780} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M9.625 9.625L7.65417 7.65417" id="Vector_2" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_955">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function TextInput1() {
  return (
    <div className="flex-[593.367_0_0] h-[16.5px] min-w-px relative" data-name="Text Input">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start justify-center overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[normal] not-italic relative shrink-0 text-[11px] text-[rgba(229,229,234,0.5)] w-full">Filter messages…</p>
      </div>
    </div>
  );
}

function SearchInput1() {
  return (
    <div className="bg-[rgba(255,255,255,0.06)] flex-[629.117_0_0] h-[28px] min-w-px relative rounded-[8.125px]" data-name="SearchInput">
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[9.125px] py-px relative size-full">
          <Icon16 />
          <TextInput1 />
        </div>
      </div>
    </div>
  );
}

function Button6() {
  return (
    <div className="bg-[rgba(255,255,255,0.1)] relative rounded-[6.125px] shrink-0" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center justify-center px-[10px] py-[4px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15.75px] not-italic relative shrink-0 text-[#e5e5ea] text-[10.5px] text-center whitespace-nowrap">All</p>
      </div>
    </div>
  );
}

function Button7() {
  return (
    <div className="relative rounded-[6.125px] shrink-0" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center justify-center px-[10px] py-[4px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-center whitespace-nowrap">User</p>
      </div>
    </div>
  );
}

function Button8() {
  return (
    <div className="relative rounded-[6.125px] shrink-0" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center justify-center px-[10px] py-[4px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-center whitespace-nowrap">Model</p>
      </div>
    </div>
  );
}

function Button9() {
  return (
    <div className="relative rounded-[6.125px] shrink-0" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center justify-center px-[10px] py-[4px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-center whitespace-nowrap">Tools</p>
      </div>
    </div>
  );
}

function Button10() {
  return (
    <div className="relative rounded-[6.125px] shrink-0" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center justify-center px-[10px] py-[4px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-center whitespace-nowrap">Agents</p>
      </div>
    </div>
  );
}

function TabBar() {
  return (
    <div className="bg-[rgba(255,255,255,0.05)] relative rounded-[8.125px] shrink-0" data-name="TabBar">
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[1.625px] items-center p-[2.625px] relative size-full">
        <Button6 />
        <Button7 />
        <Button8 />
        <Button9 />
        <Button10 />
      </div>
    </div>
  );
}

function FilterRow1() {
  return (
    <div className="bg-[rgba(28,28,30,0.8)] h-[40px] relative shrink-0" data-name="FilterRow">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center pb-px px-[9.75px] relative size-full">
        <SearchInput1 />
        <TabBar />
      </div>
    </div>
  );
}

function Icon17() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d={svgPaths.p80fa800} id="Vector" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p2b87e740} id="Vector_2" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Container72() {
  return (
    <div className="bg-[rgba(58,58,60,0.8)] relative rounded-[16777200px] shrink-0 size-[24px]" data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.1)] border-solid inset-0 pointer-events-none rounded-[16777200px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center p-px relative size-full">
        <Icon17 />
      </div>
    </div>
  );
}

function Container71() {
  return (
    <div className="h-[72.922px] relative shrink-0 w-[24px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center pt-[3.25px] relative size-full">
        <Container72 />
      </div>
    </div>
  );
}

function Text10() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[16.5px] not-italic relative shrink-0 text-[#8e8e93] text-[11px] whitespace-nowrap">User / Harness</p>
      </div>
    </div>
  );
}

function Text11() {
  return <div className="flex-[634.453_0_0] h-0 min-w-px relative" data-name="Text" />;
}

function Text12() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#3a3a3c] text-[9.5px] whitespace-nowrap">#1</p>
      </div>
    </div>
  );
}

function Text13() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">34 tok</p>
      </div>
    </div>
  );
}

function Text14() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] whitespace-nowrap">14:23:01</p>
      </div>
    </div>
  );
}

function Container74() {
  return (
    <div className="h-[21.875px] relative shrink-0 w-[835.25px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center pb-[4.875px] relative size-full">
        <Text10 />
        <Text11 />
        <Text12 />
        <Text13 />
        <Text14 />
      </div>
    </div>
  );
}

function Container75() {
  return (
    <div className="bg-[rgba(44,44,46,0.8)] relative rounded-bl-[13px] rounded-br-[13px] rounded-tl-[4.125px] rounded-tr-[13px] shrink-0" data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-bl-[13px] rounded-br-[13px] rounded-tl-[4.125px] rounded-tr-[13px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[15px] py-[11px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[19.8px] not-italic relative shrink-0 text-[#c7c7cc] text-[12px] whitespace-nowrap">Can you help me understand the current state of the authentication module and refactor it to use the new JWT middleware?</p>
      </div>
    </div>
  );
}

function Container73() {
  return (
    <div className="flex-[835.25_0_0] h-full min-w-px relative" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pb-[9.75px] relative size-full">
        <Container74 />
        <Container75 />
      </div>
    </div>
  );
}

function MessageBubble() {
  return (
    <div className="h-[79.422px] relative shrink-0 w-full" data-name="MessageBubble">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[9.75px] items-start pl-[13px] pr-[15px] py-[3.25px] relative size-full">
        <Container71 />
        <Container73 />
      </div>
    </div>
  );
}

function Container78() {
  return <div className="bg-[rgba(255,255,255,0.05)] flex-[409.781_0_0] h-px min-w-px relative" data-name="Container" />;
}

function Text15() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Semi_Bold',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[9.5px] text-[rgba(10,132,255,0.5)] tracking-[0.665px] uppercase whitespace-nowrap">Model</p>
      </div>
    </div>
  );
}

function Container79() {
  return <div className="bg-[rgba(255,255,255,0.05)] flex-[409.789_0_0] h-px min-w-px relative" data-name="Container" />;
}

function Container77() {
  return (
    <div className="h-[15px] relative shrink-0 w-[897px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[9.75px] items-center px-[13px] relative size-full">
        <Container78 />
        <Text15 />
        <Container79 />
      </div>
    </div>
  );
}

function Text16() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] whitespace-nowrap">14:23:02</p>
      </div>
    </div>
  );
}

function Text17() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">892 tok</p>
      </div>
    </div>
  );
}

function Text18() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#3a3a3c] text-[9.5px] whitespace-nowrap">#2</p>
      </div>
    </div>
  );
}

function Text19() {
  return <div className="flex-[577.578_0_0] h-0 min-w-px relative" data-name="Text" />;
}

function Text20() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">claude-opus-4-8</p>
      </div>
    </div>
  );
}

function Text21() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[16.5px] not-italic relative shrink-0 text-[#409cff] text-[11px] whitespace-nowrap">Model</p>
      </div>
    </div>
  );
}

function Container81() {
  return (
    <div className="h-[21.875px] relative shrink-0 w-[835.25px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center justify-end pb-[4.875px] relative size-full">
        <Text16 />
        <Text17 />
        <Text18 />
        <Text19 />
        <Text20 />
        <Text21 />
      </div>
    </div>
  );
}

function Container82() {
  return (
    <div className="bg-[rgba(10,84,160,0.28)] relative rounded-bl-[13px] rounded-br-[13px] rounded-tl-[13px] rounded-tr-[4.125px] shrink-0" data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.45)] border-solid inset-0 pointer-events-none rounded-bl-[13px] rounded-br-[13px] rounded-tl-[13px] rounded-tr-[4.125px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[15px] py-[11px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[19.8px] not-italic relative shrink-0 text-[#dde8ff] text-[12px] w-[706px]">{`I'll start by reading the authentication module to understand its current structure, then look at the new JWT middleware implementation.`}</p>
      </div>
    </div>
  );
}

function Text22() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">3 tool calls</p>
      </div>
    </div>
  );
}

function Container85() {
  return <div className="bg-[rgba(255,255,255,0.1)] h-[9.75px] relative shrink-0 w-px" data-name="Container" />;
}

function Container84() {
  return (
    <div className="h-[16.625px] relative shrink-0 w-[768.43px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center justify-end pb-[1.625px] relative size-full">
        <Text22 />
        <Container85 />
      </div>
    </div>
  );
}

function Icon18() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d={svgPaths.p37ffedc0} id="Vector" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p15fbe800} id="Vector_2" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M4.58333 4.125H3.66667" id="Vector_3" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M7.33333 5.95833H3.66667" id="Vector_4" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M7.33333 7.79167H3.66667" id="Vector_5" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Text23() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon18 />
      </div>
    </div>
  );
}

function Text24() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">Read</p>
      </div>
    </div>
  );
}

function Text25() {
  return (
    <div className="flex-[629.844_0_0] h-[16.5px] min-w-px relative" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#636366] text-[11px] whitespace-nowrap">/src/auth/middleware.ts</p>
      </div>
    </div>
  );
}

function Text26() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">42ms</p>
      </div>
    </div>
  );
}

function Icon19() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_907)" id="Icon">
          <path d={svgPaths.p1f658e00} id="Vector" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p246b4c00} id="Vector_2" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_907">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function StatusIcon() {
  return (
    <div className="relative shrink-0" data-name="StatusIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon19 />
      </div>
    </div>
  );
}

function Icon20() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M4.5 9L7.5 6L4.5 3" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text27() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon20 />
      </div>
    </div>
  );
}

function Container86() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <Text26 />
        <StatusIcon />
        <Text27 />
      </div>
    </div>
  );
}

function Button11() {
  return (
    <div className="relative shrink-0 w-full" data-name="Button">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[9.75px] py-[6.5px] relative size-full">
          <Text23 />
          <Text24 />
          <Text25 />
          <Container86 />
        </div>
      </div>
    </div>
  );
}

function ToolCallCard() {
  return (
    <div className="bg-[rgba(255,255,255,0.04)] h-[31.5px] relative rounded-[8.125px] shrink-0 w-full" data-name="ToolCallCard">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip p-px relative rounded-[inherit] size-full">
        <Button11 />
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
    </div>
  );
}

function Icon21() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_896)" id="Icon">
          <path d={svgPaths.p2c2cc780} id="Vector" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M9.625 9.625L7.65417 7.65417" id="Vector_2" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_896">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function Text28() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon21 />
      </div>
    </div>
  );
}

function Text29() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">Glob</p>
      </div>
    </div>
  );
}

function Text30() {
  return (
    <div className="flex-[629.844_0_0] h-[16.5px] min-w-px relative" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#636366] text-[11px] whitespace-nowrap">{`src/auth/**/*.ts`}</p>
      </div>
    </div>
  );
}

function Text31() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">18ms</p>
      </div>
    </div>
  );
}

function Icon22() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_907)" id="Icon">
          <path d={svgPaths.p1f658e00} id="Vector" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p246b4c00} id="Vector_2" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_907">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function StatusIcon1() {
  return (
    <div className="relative shrink-0" data-name="StatusIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon22 />
      </div>
    </div>
  );
}

function Icon23() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M4.5 9L7.5 6L4.5 3" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text32() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon23 />
      </div>
    </div>
  );
}

function Container87() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <Text31 />
        <StatusIcon1 />
        <Text32 />
      </div>
    </div>
  );
}

function Button12() {
  return (
    <div className="relative shrink-0 w-full" data-name="Button">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[9.75px] py-[6.5px] relative size-full">
          <Text28 />
          <Text29 />
          <Text30 />
          <Container87 />
        </div>
      </div>
    </div>
  );
}

function ToolCallCard1() {
  return (
    <div className="bg-[rgba(255,255,255,0.04)] h-[31.5px] relative rounded-[8.125px] shrink-0 w-full" data-name="ToolCallCard">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip p-px relative rounded-[inherit] size-full">
        <Button12 />
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
    </div>
  );
}

function Icon24() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d={svgPaths.p37ffedc0} id="Vector" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p15fbe800} id="Vector_2" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M4.58333 4.125H3.66667" id="Vector_3" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M7.33333 5.95833H3.66667" id="Vector_4" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M7.33333 7.79167H3.66667" id="Vector_5" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Text33() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon24 />
      </div>
    </div>
  );
}

function Text34() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">Read</p>
      </div>
    </div>
  );
}

function Text35() {
  return (
    <div className="flex-[629.844_0_0] h-[16.5px] min-w-px relative" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#636366] text-[11px] whitespace-nowrap">/src/auth/utils.ts</p>
      </div>
    </div>
  );
}

function Text36() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">38ms</p>
      </div>
    </div>
  );
}

function Icon25() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_907)" id="Icon">
          <path d={svgPaths.p1f658e00} id="Vector" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p246b4c00} id="Vector_2" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_907">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function StatusIcon2() {
  return (
    <div className="relative shrink-0" data-name="StatusIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon25 />
      </div>
    </div>
  );
}

function Icon26() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M4.5 9L7.5 6L4.5 3" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text37() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon26 />
      </div>
    </div>
  );
}

function Container88() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <Text36 />
        <StatusIcon2 />
        <Text37 />
      </div>
    </div>
  );
}

function Button13() {
  return (
    <div className="relative shrink-0 w-full" data-name="Button">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[9.75px] py-[6.5px] relative size-full">
          <Text33 />
          <Text34 />
          <Text35 />
          <Container88 />
        </div>
      </div>
    </div>
  );
}

function ToolCallCard2() {
  return (
    <div className="bg-[rgba(255,255,255,0.04)] h-[31.5px] relative rounded-[8.125px] shrink-0 w-full" data-name="ToolCallCard">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip p-px relative rounded-[inherit] size-full">
        <Button13 />
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
    </div>
  );
}

function Container83() {
  return (
    <div className="h-[132.25px] relative shrink-0 w-[768.43px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col gap-[4.875px] items-start pt-[6.5px] relative size-full">
        <Container84 />
        <ToolCallCard />
        <ToolCallCard1 />
        <ToolCallCard2 />
      </div>
    </div>
  );
}

function Container80() {
  return (
    <div className="flex-[835.25_0_0] h-full min-w-px relative" data-name="Container">
      <div className="flex flex-col items-end size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end pb-[9.75px] relative size-full">
          <Container81 />
          <Container82 />
          <Container83 />
        </div>
      </div>
    </div>
  );
}

function Icon27() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d="M5.5 3.66667V1.83333H3.66667" id="Vector" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p15892640} id="Vector_2" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M0.916667 6.41667H1.83333" id="Vector_3" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M9.16667 6.41667H10.0833" id="Vector_4" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M6.875 5.95833V6.875" id="Vector_5" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M4.125 5.95833V6.875" id="Vector_6" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Container90() {
  return (
    <div className="relative rounded-[16777200px] shrink-0 size-[24px]" style={{ backgroundImage: "linear-gradient(135deg, rgba(10, 132, 255, 0.4) 0%, rgba(94, 92, 230, 0.4) 100%)" }} data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.4)] border-solid inset-0 pointer-events-none rounded-[16777200px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center p-px relative size-full">
        <Icon27 />
      </div>
    </div>
  );
}

function Container91() {
  return <div className="bg-[rgba(10,132,255,0.18)] h-[194.469px] min-h-[12px] relative shrink-0 w-px" data-name="Container" />;
}

function ContainerMargin2() {
  return (
    <div className="flex-[197.719_0_0] min-h-px relative" data-name="Container:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[3.25px] relative size-full">
        <Container91 />
      </div>
    </div>
  );
}

function Container89() {
  return (
    <div className="h-[224.969px] relative shrink-0 w-[24px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center pt-[3.25px] relative size-full">
        <Container90 />
        <ContainerMargin2 />
      </div>
    </div>
  );
}

function MessageBubble1() {
  return (
    <div className="bg-[rgba(255,255,255,0.02)] h-[231.469px] relative shrink-0 w-full" data-name="MessageBubble">
      <div aria-hidden className="absolute border-[#0a84ff] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row justify-end size-full">
        <div className="content-stretch flex gap-[9.75px] items-start justify-end pl-[13px] pr-[15px] py-[3.25px] relative size-full">
          <Container80 />
          <Container89 />
        </div>
      </div>
    </div>
  );
}

function MessageBubbleMargin() {
  return (
    <div className="relative shrink-0 w-full" data-name="MessageBubble:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[8.125px] relative size-full">
        <MessageBubble1 />
      </div>
    </div>
  );
}

function Container76() {
  return (
    <div className="relative shrink-0 w-[897px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[8.125px] relative size-full">
        <Container77 />
        <MessageBubbleMargin />
      </div>
    </div>
  );
}

function Text38() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] whitespace-nowrap">14:23:05</p>
      </div>
    </div>
  );
}

function Text39() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">1,204 tok</p>
      </div>
    </div>
  );
}

function Text40() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#3a3a3c] text-[9.5px] whitespace-nowrap">#3</p>
      </div>
    </div>
  );
}

function Text41() {
  return <div className="flex-[565.531_0_0] h-0 min-w-px relative" data-name="Text" />;
}

function Text42() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">claude-opus-4-8</p>
      </div>
    </div>
  );
}

function Text43() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[16.5px] not-italic relative shrink-0 text-[#409cff] text-[11px] whitespace-nowrap">Model</p>
      </div>
    </div>
  );
}

function Container93() {
  return (
    <div className="h-[21.875px] relative shrink-0 w-[835.25px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center justify-end pb-[4.875px] relative size-full">
        <Text38 />
        <Text39 />
        <Text40 />
        <Text41 />
        <Text42 />
        <Text43 />
      </div>
    </div>
  );
}

function Container94() {
  return (
    <div className="bg-[rgba(10,84,160,0.16)] relative rounded-bl-[13px] rounded-br-[13px] rounded-tl-[13px] rounded-tr-[4.125px] shrink-0" data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.2)] border-solid inset-0 pointer-events-none rounded-bl-[13px] rounded-br-[13px] rounded-tl-[13px] rounded-tr-[4.125px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[15px] py-[11px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[19.8px] not-italic relative shrink-0 text-[#dde8ff] text-[12px] w-[706px]">I can see the issue — the legacy auth uses a custom `x-auth-token` header and inline verification. The new JWT middleware uses `Authorization: Bearer` and has proper error handling. Let me check if there are any callers before refactoring.</p>
      </div>
    </div>
  );
}

function Text44() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">1 tool call</p>
      </div>
    </div>
  );
}

function Container97() {
  return <div className="bg-[rgba(255,255,255,0.1)] h-[9.75px] relative shrink-0 w-px" data-name="Container" />;
}

function Container96() {
  return (
    <div className="h-[16.625px] relative shrink-0 w-[768.43px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center justify-end pb-[1.625px] relative size-full">
        <Text44 />
        <Container97 />
      </div>
    </div>
  );
}

function Icon28() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d="M1.83333 4.125H9.16667" id="Vector" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M1.83333 6.875H9.16667" id="Vector_2" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M4.58333 1.375L3.66667 9.625" id="Vector_3" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M7.33333 1.375L6.41667 9.625" id="Vector_4" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Text45() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon28 />
      </div>
    </div>
  );
}

function Text46() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">Grep</p>
      </div>
    </div>
  );
}

function Text47() {
  return (
    <div className="flex-[623.828_0_0] h-[16.5px] min-w-px relative" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#636366] text-[11px] whitespace-nowrap">legacyAuth</p>
      </div>
    </div>
  );
}

function Text48() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">156ms</p>
      </div>
    </div>
  );
}

function Icon29() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_907)" id="Icon">
          <path d={svgPaths.p1f658e00} id="Vector" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p246b4c00} id="Vector_2" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_907">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function StatusIcon3() {
  return (
    <div className="relative shrink-0" data-name="StatusIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon29 />
      </div>
    </div>
  );
}

function Icon30() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M4.5 9L7.5 6L4.5 3" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text49() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon30 />
      </div>
    </div>
  );
}

function Container98() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <Text48 />
        <StatusIcon3 />
        <Text49 />
      </div>
    </div>
  );
}

function Button14() {
  return (
    <div className="relative shrink-0 w-full" data-name="Button">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[9.75px] py-[6.5px] relative size-full">
          <Text45 />
          <Text46 />
          <Text47 />
          <Container98 />
        </div>
      </div>
    </div>
  );
}

function ToolCallCard3() {
  return (
    <div className="bg-[rgba(255,255,255,0.04)] h-[31.5px] relative rounded-[8.125px] shrink-0 w-full" data-name="ToolCallCard">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip p-px relative rounded-[inherit] size-full">
        <Button14 />
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
    </div>
  );
}

function Container95() {
  return (
    <div className="h-[59.5px] relative shrink-0 w-[768.43px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col gap-[4.875px] items-start pt-[6.5px] relative size-full">
        <Container96 />
        <ToolCallCard3 />
      </div>
    </div>
  );
}

function Container92() {
  return (
    <div className="flex-[835.25_0_0] h-full min-w-px relative" data-name="Container">
      <div className="flex flex-col items-end size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end pb-[9.75px] relative size-full">
          <Container93 />
          <Container94 />
          <Container95 />
        </div>
      </div>
    </div>
  );
}

function Icon31() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d="M5.5 3.66667V1.83333H3.66667" id="Vector" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p15892640} id="Vector_2" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M0.916667 6.41667H1.83333" id="Vector_3" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M9.16667 6.41667H10.0833" id="Vector_4" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M6.875 5.95833V6.875" id="Vector_5" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M4.125 5.95833V6.875" id="Vector_6" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Container100() {
  return (
    <div className="relative rounded-[16777200px] shrink-0 size-[24px]" style={{ backgroundImage: "linear-gradient(135deg, rgba(10, 132, 255, 0.4) 0%, rgba(94, 92, 230, 0.4) 100%)" }} data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.4)] border-solid inset-0 pointer-events-none rounded-[16777200px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center p-px relative size-full">
        <Icon31 />
      </div>
    </div>
  );
}

function Container101() {
  return <div className="bg-[rgba(10,132,255,0.18)] h-[121.719px] min-h-[12px] relative shrink-0 w-px" data-name="Container" />;
}

function ContainerMargin3() {
  return (
    <div className="flex-[124.969_0_0] min-h-px relative" data-name="Container:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[3.25px] relative size-full">
        <Container101 />
      </div>
    </div>
  );
}

function Container99() {
  return (
    <div className="h-[152.219px] relative shrink-0 w-[24px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center pt-[3.25px] relative size-full">
        <Container100 />
        <ContainerMargin3 />
      </div>
    </div>
  );
}

function MessageBubble2() {
  return (
    <div className="h-[158.719px] relative shrink-0 w-full" data-name="MessageBubble">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row justify-end size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[9.75px] items-start justify-end pl-[13px] pr-[15px] py-[3.25px] relative size-full">
          <Container92 />
          <Container99 />
        </div>
      </div>
    </div>
  );
}

function Text50() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] whitespace-nowrap">14:23:08</p>
      </div>
    </div>
  );
}

function Text51() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">1,589 tok</p>
      </div>
    </div>
  );
}

function Text52() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#3a3a3c] text-[9.5px] whitespace-nowrap">#4</p>
      </div>
    </div>
  );
}

function Text53() {
  return <div className="flex-[565.531_0_0] h-0 min-w-px relative" data-name="Text" />;
}

function Text54() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">claude-opus-4-8</p>
      </div>
    </div>
  );
}

function Text55() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[16.5px] not-italic relative shrink-0 text-[#409cff] text-[11px] whitespace-nowrap">Model</p>
      </div>
    </div>
  );
}

function Container103() {
  return (
    <div className="h-[21.875px] relative shrink-0 w-[835.25px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center justify-end pb-[4.875px] relative size-full">
        <Text50 />
        <Text51 />
        <Text52 />
        <Text53 />
        <Text54 />
        <Text55 />
      </div>
    </div>
  );
}

function Container104() {
  return (
    <div className="bg-[rgba(10,84,160,0.16)] relative rounded-bl-[13px] rounded-br-[13px] rounded-tl-[13px] rounded-tr-[4.125px] shrink-0" data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.2)] border-solid inset-0 pointer-events-none rounded-bl-[13px] rounded-br-[13px] rounded-tl-[13px] rounded-tr-[4.125px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[15px] py-[11px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[19.8px] not-italic relative shrink-0 text-[#dde8ff] text-[12px] w-[706px]">{`Found 4 call sites across 3 files. I'll delegate the analysis of the admin routes to a subagent while I handle the user and API routes directly.`}</p>
      </div>
    </div>
  );
}

function Icon32() {
  return (
    <div className="relative shrink-0 size-[13px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 13 13">
        <g id="Icon">
          <path d="M3.25 1.625V8.125" id="Vector" stroke="var(--stroke-0, #0A84FF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.08333" />
          <path d={svgPaths.p3dee5670} id="Vector_2" stroke="var(--stroke-0, #0A84FF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.08333" />
          <path d={svgPaths.p116815f0} id="Vector_3" stroke="var(--stroke-0, #0A84FF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.08333" />
          <path d={svgPaths.p11d58680} id="Vector_4" stroke="var(--stroke-0, #0A84FF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.08333" />
        </g>
      </svg>
    </div>
  );
}

function Container106() {
  return (
    <div className="bg-[rgba(10,132,255,0.15)] relative rounded-[8.125px] shrink-0 size-[28px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center relative size-full">
        <Icon32 />
      </div>
    </div>
  );
}

function Text56() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[15px] not-italic relative shrink-0 text-[#0a84ff] text-[10px] tracking-[0.8px] uppercase whitespace-nowrap">Subagent</p>
      </div>
    </div>
  );
}

function Text57() {
  return (
    <div className="bg-[rgba(48,209,88,0.1)] relative rounded-[4px] shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[5px] py-[2px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[13.5px] not-italic relative shrink-0 text-[#30d158] text-[9px] whitespace-nowrap">success</p>
      </div>
    </div>
  );
}

function Container108() {
  return (
    <div className="relative shrink-0 w-full" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <Text56 />
        <Text57 />
      </div>
    </div>
  );
}

function Paragraph() {
  return (
    <div className="content-stretch flex flex-col h-[15.75px] items-start overflow-clip relative shrink-0 w-full" data-name="Paragraph">
      <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#aeaeb2] text-[10.5px] whitespace-nowrap">Analyze src/routes/admin.ts and update legacyAuth to use jwtMiddlewa…</p>
    </div>
  );
}

function ParagraphMargin() {
  return (
    <div className="relative shrink-0 w-full" data-name="Paragraph:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[1.625px] relative size-full">
        <Paragraph />
      </div>
    </div>
  );
}

function Container107() {
  return (
    <div className="flex-[710.805_0_0] min-w-px relative" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Container108 />
        <ParagraphMargin />
      </div>
    </div>
  );
}

function Container105() {
  return (
    <div className="relative shrink-0 w-full" data-name="Container">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[8.125px] items-center px-[9.75px] py-[8.125px] relative size-full">
          <Container106 />
          <Container107 />
        </div>
      </div>
    </div>
  );
}

function ModelBadge11() {
  return (
    <div className="bg-[rgba(10,132,255,0.12)] relative rounded-[5px] shrink-0" data-name="ModelBadge">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.25)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[7px] py-[3px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#409cff] text-[9.5px] whitespace-nowrap">sonnet 4.6</p>
      </div>
    </div>
  );
}

function Icon33() {
  return (
    <div className="relative shrink-0 size-[10px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 10 10">
        <g clipPath="url(#clip0_1_883)" id="Icon">
          <path d={svgPaths.p21613c00} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.833333" />
          <path d={svgPaths.p3011a400} id="Vector_2" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.833333" />
          <path d={svgPaths.p310d1880} id="Vector_3" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.833333" />
        </g>
        <defs>
          <clipPath id="clip0_1_883">
            <rect fill="white" height="10" width="10" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function Text58() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">6 turns</p>
      </div>
    </div>
  );
}

function Container110() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[3.25px] items-center relative size-full">
        <Icon33 />
        <Text58 />
      </div>
    </div>
  );
}

function Icon34() {
  return (
    <div className="relative shrink-0 size-[10px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 10 10">
        <g clipPath="url(#clip0_1_973)" id="Icon">
          <path d={svgPaths.p3cf7650} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.833333" />
          <path d="M5 2.5V5L6.66667 5.83333" id="Vector_2" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.833333" />
        </g>
        <defs>
          <clipPath id="clip0_1_973">
            <rect fill="white" height="10" width="10" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function Text59() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">8.4s</p>
      </div>
    </div>
  );
}

function Container111() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[3.25px] items-center relative size-full">
        <Icon34 />
        <Text59 />
      </div>
    </div>
  );
}

function Icon35() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d={svgPaths.p25b4b700} id="Vector" stroke="var(--stroke-0, #0A84FF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.pe156380} id="Vector_2" stroke="var(--stroke-0, #0A84FF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Button15() {
  return (
    <div className="content-stretch flex gap-[4.875px] items-center px-[8px] py-[3px] relative rounded-[8.125px] shrink-0" data-name="Button">
      <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15px] not-italic relative shrink-0 text-[#0a84ff] text-[10px] text-center whitespace-nowrap">Follow trace</p>
      <Icon35 />
    </div>
  );
}

function ButtonAlign() {
  return (
    <div className="flex-[1_0_0] min-w-px relative" data-name="Button:align">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-start justify-end relative size-full">
        <Button15 />
      </div>
    </div>
  );
}

function Container109() {
  return (
    <div className="relative shrink-0 w-full" data-name="Container">
      <div aria-hidden className="absolute border-[rgba(10,132,255,0.1)] border-solid border-t inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[13px] items-center pb-[6.5px] pt-[7.5px] px-[9.75px] relative size-full">
          <ModelBadge11 />
          <Container110 />
          <Container111 />
          <ButtonAlign />
        </div>
      </div>
    </div>
  );
}

function SubagentCard() {
  return (
    <div className="h-[88.125px] relative rounded-[12.125px] shrink-0 w-[768.43px]" style={{ backgroundImage: "linear-gradient(173.458deg, rgba(10, 132, 255, 0.08) 0%, rgba(10, 132, 255, 0.03) 100%)" }} data-name="SubagentCard">
      <div className="content-stretch flex flex-col items-start overflow-clip p-px relative rounded-[inherit] size-full">
        <Container105 />
        <Container109 />
      </div>
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.22)] border-solid inset-0 pointer-events-none rounded-[12.125px]" />
    </div>
  );
}

function ContainerMargin4() {
  return (
    <div className="relative shrink-0" data-name="Container:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[6.5px] relative size-full">
        <SubagentCard />
      </div>
    </div>
  );
}

function Container102() {
  return (
    <div className="flex-[835.25_0_0] h-full min-w-px relative" data-name="Container">
      <div className="flex flex-col items-end size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end pb-[9.75px] relative size-full">
          <Container103 />
          <Container104 />
          <ContainerMargin4 />
        </div>
      </div>
    </div>
  );
}

function Icon36() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d="M5.5 3.66667V1.83333H3.66667" id="Vector" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p15892640} id="Vector_2" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M0.916667 6.41667H1.83333" id="Vector_3" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M9.16667 6.41667H10.0833" id="Vector_4" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M6.875 5.95833V6.875" id="Vector_5" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M4.125 5.95833V6.875" id="Vector_6" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Container113() {
  return (
    <div className="relative rounded-[16777200px] shrink-0 size-[24px]" style={{ backgroundImage: "linear-gradient(135deg, rgba(10, 132, 255, 0.4) 0%, rgba(94, 92, 230, 0.4) 100%)" }} data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.4)] border-solid inset-0 pointer-events-none rounded-[16777200px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center p-px relative size-full">
        <Icon36 />
      </div>
    </div>
  );
}

function Container114() {
  return <div className="bg-[rgba(10,132,255,0.18)] h-[156.844px] min-h-[12px] relative shrink-0 w-px" data-name="Container" />;
}

function ContainerMargin5() {
  return (
    <div className="flex-[160.094_0_0] min-h-px relative" data-name="Container:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[3.25px] relative size-full">
        <Container114 />
      </div>
    </div>
  );
}

function Container112() {
  return (
    <div className="h-[187.344px] relative shrink-0 w-[24px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center pt-[3.25px] relative size-full">
        <Container113 />
        <ContainerMargin5 />
      </div>
    </div>
  );
}

function MessageBubble3() {
  return (
    <div className="h-[193.844px] relative shrink-0 w-full" data-name="MessageBubble">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row justify-end size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[9.75px] items-start justify-end pl-[13px] pr-[15px] py-[3.25px] relative size-full">
          <Container102 />
          <Container112 />
        </div>
      </div>
    </div>
  );
}

function Text60() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] whitespace-nowrap">14:23:09</p>
      </div>
    </div>
  );
}

function Text61() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">1,820 tok</p>
      </div>
    </div>
  );
}

function Text62() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#3a3a3c] text-[9.5px] whitespace-nowrap">#5</p>
      </div>
    </div>
  );
}

function Text63() {
  return <div className="flex-[565.531_0_0] h-0 min-w-px relative" data-name="Text" />;
}

function Text64() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">claude-opus-4-8</p>
      </div>
    </div>
  );
}

function Text65() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[16.5px] not-italic relative shrink-0 text-[#409cff] text-[11px] whitespace-nowrap">Model</p>
      </div>
    </div>
  );
}

function Container116() {
  return (
    <div className="h-[21.875px] relative shrink-0 w-[835.25px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center justify-end pb-[4.875px] relative size-full">
        <Text60 />
        <Text61 />
        <Text62 />
        <Text63 />
        <Text64 />
        <Text65 />
      </div>
    </div>
  );
}

function Container117() {
  return (
    <div className="bg-[rgba(10,84,160,0.16)] relative rounded-bl-[13px] rounded-br-[13px] rounded-tl-[13px] rounded-tr-[4.125px] shrink-0" data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.2)] border-solid inset-0 pointer-events-none rounded-bl-[13px] rounded-br-[13px] rounded-tl-[13px] rounded-tr-[4.125px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[15px] py-[11px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[19.8px] not-italic relative shrink-0 text-[#dde8ff] text-[12px] whitespace-nowrap">{`While the subagent handles admin routes, I'll update the user and API routes.`}</p>
      </div>
    </div>
  );
}

function Text66() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">3 tool calls</p>
      </div>
    </div>
  );
}

function Container120() {
  return <div className="bg-[rgba(255,255,255,0.1)] h-[9.75px] relative shrink-0 w-px" data-name="Container" />;
}

function Container119() {
  return (
    <div className="h-[16.625px] relative shrink-0 w-[768.43px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center justify-end pb-[1.625px] relative size-full">
        <Text66 />
        <Container120 />
      </div>
    </div>
  );
}

function Icon37() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d={svgPaths.p264f2880} id="Vector" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p2720fb80} id="Vector_2" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p16746a80} id="Vector_3" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Text67() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon37 />
      </div>
    </div>
  );
}

function Text68() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">Edit</p>
      </div>
    </div>
  );
}

function Text69() {
  return (
    <div className="flex-[629.844_0_0] h-[16.5px] min-w-px relative" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#636366] text-[11px] whitespace-nowrap">src/routes/user.ts</p>
      </div>
    </div>
  );
}

function Text70() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">61ms</p>
      </div>
    </div>
  );
}

function Icon38() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_907)" id="Icon">
          <path d={svgPaths.p1f658e00} id="Vector" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p246b4c00} id="Vector_2" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_907">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function StatusIcon4() {
  return (
    <div className="relative shrink-0" data-name="StatusIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon38 />
      </div>
    </div>
  );
}

function Icon39() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M4.5 9L7.5 6L4.5 3" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text71() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon39 />
      </div>
    </div>
  );
}

function Container121() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <Text70 />
        <StatusIcon4 />
        <Text71 />
      </div>
    </div>
  );
}

function Button16() {
  return (
    <div className="relative shrink-0 w-full" data-name="Button">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[9.75px] py-[6.5px] relative size-full">
          <Text67 />
          <Text68 />
          <Text69 />
          <Container121 />
        </div>
      </div>
    </div>
  );
}

function ToolCallCard4() {
  return (
    <div className="bg-[rgba(255,255,255,0.04)] h-[31.5px] relative rounded-[8.125px] shrink-0 w-full" data-name="ToolCallCard">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip p-px relative rounded-[inherit] size-full">
        <Button16 />
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
    </div>
  );
}

function Icon40() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d={svgPaths.p264f2880} id="Vector" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p2720fb80} id="Vector_2" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p16746a80} id="Vector_3" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Text72() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon40 />
      </div>
    </div>
  );
}

function Text73() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">Edit</p>
      </div>
    </div>
  );
}

function Text74() {
  return (
    <div className="flex-[629.844_0_0] h-[16.5px] min-w-px relative" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#636366] text-[11px] whitespace-nowrap">src/routes/api.ts</p>
      </div>
    </div>
  );
}

function Text75() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">55ms</p>
      </div>
    </div>
  );
}

function Icon41() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_907)" id="Icon">
          <path d={svgPaths.p1f658e00} id="Vector" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p246b4c00} id="Vector_2" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_907">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function StatusIcon5() {
  return (
    <div className="relative shrink-0" data-name="StatusIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon41 />
      </div>
    </div>
  );
}

function Icon42() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M4.5 9L7.5 6L4.5 3" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text76() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon42 />
      </div>
    </div>
  );
}

function Container122() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <Text75 />
        <StatusIcon5 />
        <Text76 />
      </div>
    </div>
  );
}

function Button17() {
  return (
    <div className="relative shrink-0 w-full" data-name="Button">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[9.75px] py-[6.5px] relative size-full">
          <Text72 />
          <Text73 />
          <Text74 />
          <Container122 />
        </div>
      </div>
    </div>
  );
}

function ToolCallCard5() {
  return (
    <div className="bg-[rgba(255,255,255,0.04)] h-[31.5px] relative rounded-[8.125px] shrink-0 w-full" data-name="ToolCallCard">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip p-px relative rounded-[inherit] size-full">
        <Button17 />
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
    </div>
  );
}

function Icon43() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d={svgPaths.p35217f80} id="Vector" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M5.5 8.70833H9.16667" id="Vector_2" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Text77() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon43 />
      </div>
    </div>
  );
}

function Text78() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">Bash</p>
      </div>
    </div>
  );
}

function Text79() {
  return (
    <div className="flex-[617.805_0_0] h-[16.5px] min-w-px relative" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#636366] text-[11px] whitespace-nowrap">npm test -- --testPathPattern=auth</p>
      </div>
    </div>
  );
}

function Text80() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">3241ms</p>
      </div>
    </div>
  );
}

function Icon44() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_907)" id="Icon">
          <path d={svgPaths.p1f658e00} id="Vector" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p246b4c00} id="Vector_2" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_907">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function StatusIcon6() {
  return (
    <div className="relative shrink-0" data-name="StatusIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon44 />
      </div>
    </div>
  );
}

function Icon45() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M4.5 9L7.5 6L4.5 3" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text81() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon45 />
      </div>
    </div>
  );
}

function Container123() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <Text80 />
        <StatusIcon6 />
        <Text81 />
      </div>
    </div>
  );
}

function Button18() {
  return (
    <div className="relative shrink-0 w-full" data-name="Button">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[9.75px] py-[6.5px] relative size-full">
          <Text77 />
          <Text78 />
          <Text79 />
          <Container123 />
        </div>
      </div>
    </div>
  );
}

function ToolCallCard6() {
  return (
    <div className="bg-[rgba(255,255,255,0.04)] h-[31.5px] relative rounded-[8.125px] shrink-0 w-full" data-name="ToolCallCard">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip p-px relative rounded-[inherit] size-full">
        <Button18 />
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
    </div>
  );
}

function Container118() {
  return (
    <div className="h-[132.25px] relative shrink-0 w-[768.43px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col gap-[4.875px] items-start pt-[6.5px] relative size-full">
        <Container119 />
        <ToolCallCard4 />
        <ToolCallCard5 />
        <ToolCallCard6 />
      </div>
    </div>
  );
}

function Container115() {
  return (
    <div className="flex-[835.25_0_0] h-full min-w-px relative" data-name="Container">
      <div className="flex flex-col items-end size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end pb-[9.75px] relative size-full">
          <Container116 />
          <Container117 />
          <Container118 />
        </div>
      </div>
    </div>
  );
}

function Icon46() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d="M5.5 3.66667V1.83333H3.66667" id="Vector" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p15892640} id="Vector_2" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M0.916667 6.41667H1.83333" id="Vector_3" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M9.16667 6.41667H10.0833" id="Vector_4" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M6.875 5.95833V6.875" id="Vector_5" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M4.125 5.95833V6.875" id="Vector_6" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Container125() {
  return (
    <div className="relative rounded-[16777200px] shrink-0 size-[24px]" style={{ backgroundImage: "linear-gradient(135deg, rgba(10, 132, 255, 0.4) 0%, rgba(94, 92, 230, 0.4) 100%)" }} data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.4)] border-solid inset-0 pointer-events-none rounded-[16777200px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center p-px relative size-full">
        <Icon46 />
      </div>
    </div>
  );
}

function Container124() {
  return (
    <div className="h-[205.172px] relative shrink-0 w-[24px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center pt-[3.25px] relative size-full">
        <Container125 />
      </div>
    </div>
  );
}

function MessageBubble4() {
  return (
    <div className="h-[211.672px] relative shrink-0 w-full" data-name="MessageBubble">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row justify-end size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[9.75px] items-start justify-end pl-[13px] pr-[15px] py-[3.25px] relative size-full">
          <Container115 />
          <Container124 />
        </div>
      </div>
    </div>
  );
}

function Container128() {
  return <div className="bg-[rgba(255,255,255,0.05)] flex-[412.977_0_0] h-px min-w-px relative" data-name="Container" />;
}

function Text82() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Semi_Bold',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#636366] text-[9.5px] tracking-[0.665px] uppercase whitespace-nowrap">User</p>
      </div>
    </div>
  );
}

function Container129() {
  return <div className="bg-[rgba(255,255,255,0.05)] flex-[412.984_0_0] h-px min-w-px relative" data-name="Container" />;
}

function Container127() {
  return (
    <div className="h-[15px] relative shrink-0 w-[897px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[9.75px] items-center px-[13px] relative size-full">
        <Container128 />
        <Text82 />
        <Container129 />
      </div>
    </div>
  );
}

function Icon47() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d={svgPaths.p80fa800} id="Vector" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p2b87e740} id="Vector_2" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Container131() {
  return (
    <div className="bg-[rgba(58,58,60,0.8)] relative rounded-[16777200px] shrink-0 size-[24px]" data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.1)] border-solid inset-0 pointer-events-none rounded-[16777200px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center p-px relative size-full">
        <Icon47 />
      </div>
    </div>
  );
}

function Container130() {
  return (
    <div className="h-[72.922px] relative shrink-0 w-[24px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center pt-[3.25px] relative size-full">
        <Container131 />
      </div>
    </div>
  );
}

function Text83() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[16.5px] not-italic relative shrink-0 text-[#8e8e93] text-[11px] whitespace-nowrap">User / Harness</p>
      </div>
    </div>
  );
}

function Text84() {
  return <div className="flex-[634.453_0_0] h-0 min-w-px relative" data-name="Text" />;
}

function Text85() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#3a3a3c] text-[9.5px] whitespace-nowrap">#6</p>
      </div>
    </div>
  );
}

function Text86() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">28 tok</p>
      </div>
    </div>
  );
}

function Text87() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] whitespace-nowrap">14:23:45</p>
      </div>
    </div>
  );
}

function Container133() {
  return (
    <div className="h-[21.875px] relative shrink-0 w-[835.25px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center pb-[4.875px] relative size-full">
        <Text83 />
        <Text84 />
        <Text85 />
        <Text86 />
        <Text87 />
      </div>
    </div>
  );
}

function Container134() {
  return (
    <div className="bg-[rgba(44,44,46,0.8)] relative rounded-bl-[13px] rounded-br-[13px] rounded-tl-[4.125px] rounded-tr-[13px] shrink-0" data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-bl-[13px] rounded-br-[13px] rounded-tl-[4.125px] rounded-tr-[13px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[15px] py-[11px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[19.8px] not-italic relative shrink-0 text-[#c7c7cc] text-[12px] whitespace-nowrap">The tests are passing. Can you also update the API documentation to reflect the new Authorization header format?</p>
      </div>
    </div>
  );
}

function Container132() {
  return (
    <div className="flex-[835.25_0_0] h-full min-w-px relative" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pb-[9.75px] relative size-full">
        <Container133 />
        <Container134 />
      </div>
    </div>
  );
}

function MessageBubble5() {
  return (
    <div className="h-[79.422px] relative shrink-0 w-full" data-name="MessageBubble">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="content-stretch flex gap-[9.75px] items-start pl-[13px] pr-[15px] py-[3.25px] relative size-full">
        <Container130 />
        <Container132 />
      </div>
    </div>
  );
}

function MessageBubbleMargin1() {
  return (
    <div className="relative shrink-0 w-full" data-name="MessageBubble:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[8.125px] relative size-full">
        <MessageBubble5 />
      </div>
    </div>
  );
}

function Container126() {
  return (
    <div className="h-[110.672px] relative shrink-0 w-[897px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[8.125px] relative size-full">
        <Container127 />
        <MessageBubbleMargin1 />
      </div>
    </div>
  );
}

function Container137() {
  return <div className="bg-[rgba(255,255,255,0.05)] flex-[409.781_0_0] h-px min-w-px relative" data-name="Container" />;
}

function Text88() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Semi_Bold',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[9.5px] text-[rgba(10,132,255,0.5)] tracking-[0.665px] uppercase whitespace-nowrap">Model</p>
      </div>
    </div>
  );
}

function Container138() {
  return <div className="bg-[rgba(255,255,255,0.05)] flex-[409.789_0_0] h-px min-w-px relative" data-name="Container" />;
}

function Container136() {
  return (
    <div className="h-[15px] relative shrink-0 w-[897px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[9.75px] items-center px-[13px] relative size-full">
        <Container137 />
        <Text88 />
        <Container138 />
      </div>
    </div>
  );
}

function Text89() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#3a3a3c] text-[10px] whitespace-nowrap">14:23:46</p>
      </div>
    </div>
  );
}

function Text90() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">2,103 tok</p>
      </div>
    </div>
  );
}

function Text91() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#3a3a3c] text-[9.5px] whitespace-nowrap">#7</p>
      </div>
    </div>
  );
}

function Text92() {
  return <div className="flex-[565.531_0_0] h-0 min-w-px relative" data-name="Text" />;
}

function Text93() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">claude-opus-4-8</p>
      </div>
    </div>
  );
}

function Text94() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[16.5px] not-italic relative shrink-0 text-[#409cff] text-[11px] whitespace-nowrap">Model</p>
      </div>
    </div>
  );
}

function Container140() {
  return (
    <div className="h-[21.875px] relative shrink-0 w-[835.25px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center justify-end pb-[4.875px] relative size-full">
        <Text89 />
        <Text90 />
        <Text91 />
        <Text92 />
        <Text93 />
        <Text94 />
      </div>
    </div>
  );
}

function Container141() {
  return (
    <div className="bg-[rgba(10,84,160,0.16)] relative rounded-bl-[13px] rounded-br-[13px] rounded-tl-[13px] rounded-tr-[4.125px] shrink-0" data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.2)] border-solid inset-0 pointer-events-none rounded-bl-[13px] rounded-br-[13px] rounded-tl-[13px] rounded-tr-[4.125px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[15px] py-[11px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[19.8px] not-italic relative shrink-0 text-[#dde8ff] text-[12px] w-[706px]">{`Sure — I'll find and update the API docs to document \`Authorization: Bearer <token>\` instead of the legacy \`x-auth-token\` header.`}</p>
      </div>
    </div>
  );
}

function Text95() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">3 tool calls</p>
      </div>
    </div>
  );
}

function Container144() {
  return <div className="bg-[rgba(255,255,255,0.1)] h-[9.75px] relative shrink-0 w-px" data-name="Container" />;
}

function Container143() {
  return (
    <div className="h-[16.625px] relative shrink-0 w-[768.43px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center justify-end pb-[1.625px] relative size-full">
        <Text95 />
        <Container144 />
      </div>
    </div>
  );
}

function Icon48() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_896)" id="Icon">
          <path d={svgPaths.p2c2cc780} id="Vector" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M9.625 9.625L7.65417 7.65417" id="Vector_2" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_896">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function Text96() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon48 />
      </div>
    </div>
  );
}

function Text97() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">Glob</p>
      </div>
    </div>
  );
}

function Text98() {
  return (
    <div className="flex-[629.844_0_0] h-[16.5px] min-w-px relative" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#636366] text-[11px] whitespace-nowrap">{`docs/**/*.md`}</p>
      </div>
    </div>
  );
}

function Text99() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">22ms</p>
      </div>
    </div>
  );
}

function Icon49() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_907)" id="Icon">
          <path d={svgPaths.p1f658e00} id="Vector" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p246b4c00} id="Vector_2" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_907">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function StatusIcon7() {
  return (
    <div className="relative shrink-0" data-name="StatusIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon49 />
      </div>
    </div>
  );
}

function Icon50() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M4.5 9L7.5 6L4.5 3" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text100() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon50 />
      </div>
    </div>
  );
}

function Container145() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <Text99 />
        <StatusIcon7 />
        <Text100 />
      </div>
    </div>
  );
}

function Button19() {
  return (
    <div className="relative shrink-0 w-full" data-name="Button">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[9.75px] py-[6.5px] relative size-full">
          <Text96 />
          <Text97 />
          <Text98 />
          <Container145 />
        </div>
      </div>
    </div>
  );
}

function ToolCallCard7() {
  return (
    <div className="bg-[rgba(255,255,255,0.04)] h-[31.5px] relative rounded-[8.125px] shrink-0 w-full" data-name="ToolCallCard">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip p-px relative rounded-[inherit] size-full">
        <Button19 />
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
    </div>
  );
}

function Icon51() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d={svgPaths.p264f2880} id="Vector" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p2720fb80} id="Vector_2" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p16746a80} id="Vector_3" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Text101() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon51 />
      </div>
    </div>
  );
}

function Text102() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">Edit</p>
      </div>
    </div>
  );
}

function Text103() {
  return (
    <div className="flex-[629.844_0_0] h-[16.5px] min-w-px relative" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#636366] text-[11px] whitespace-nowrap">docs/api/auth.md</p>
      </div>
    </div>
  );
}

function Text104() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">44ms</p>
      </div>
    </div>
  );
}

function Icon52() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_907)" id="Icon">
          <path d={svgPaths.p1f658e00} id="Vector" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p246b4c00} id="Vector_2" stroke="var(--stroke-0, #30D158)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_907">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function StatusIcon8() {
  return (
    <div className="relative shrink-0" data-name="StatusIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon52 />
      </div>
    </div>
  );
}

function Icon53() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M4.5 9L7.5 6L4.5 3" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text105() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon53 />
      </div>
    </div>
  );
}

function Container146() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <Text104 />
        <StatusIcon8 />
        <Text105 />
      </div>
    </div>
  );
}

function Button20() {
  return (
    <div className="relative shrink-0 w-full" data-name="Button">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[9.75px] py-[6.5px] relative size-full">
          <Text101 />
          <Text102 />
          <Text103 />
          <Container146 />
        </div>
      </div>
    </div>
  );
}

function ToolCallCard8() {
  return (
    <div className="bg-[rgba(255,255,255,0.04)] h-[31.5px] relative rounded-[8.125px] shrink-0 w-full" data-name="ToolCallCard">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip p-px relative rounded-[inherit] size-full">
        <Button20 />
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
    </div>
  );
}

function Icon54() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d={svgPaths.p264f2880} id="Vector" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p2720fb80} id="Vector_2" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p16746a80} id="Vector_3" stroke="var(--stroke-0, #8E8E93)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Text106() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon54 />
      </div>
    </div>
  );
}

function Text107() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">Edit</p>
      </div>
    </div>
  );
}

function Text108() {
  return (
    <div className="flex-[629.844_0_0] h-[16.5px] min-w-px relative" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[16.5px] not-italic relative shrink-0 text-[#636366] text-[11px] whitespace-nowrap">docs/guides/authentication.md</p>
      </div>
    </div>
  );
}

function Text109() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">39ms</p>
      </div>
    </div>
  );
}

function Icon55() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_950)" id="Icon">
          <path d={svgPaths.p1f658e00} id="Vector" stroke="var(--stroke-0, #FF453A)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M6.875 4.125L4.125 6.875" id="Vector_2" stroke="var(--stroke-0, #FF453A)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M4.125 4.125L6.875 6.875" id="Vector_3" stroke="var(--stroke-0, #FF453A)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_950">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function StatusIcon9() {
  return (
    <div className="relative shrink-0" data-name="StatusIcon">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon55 />
      </div>
    </div>
  );
}

function Icon56() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M4.5 9L7.5 6L4.5 3" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text110() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon56 />
      </div>
    </div>
  );
}

function Container147() {
  return (
    <div className="relative shrink-0" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center relative size-full">
        <Text109 />
        <StatusIcon9 />
        <Text110 />
      </div>
    </div>
  );
}

function Button21() {
  return (
    <div className="relative shrink-0 w-full" data-name="Button">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[9.75px] py-[6.5px] relative size-full">
          <Text106 />
          <Text107 />
          <Text108 />
          <Container147 />
        </div>
      </div>
    </div>
  );
}

function ToolCallCard9() {
  return (
    <div className="bg-[rgba(255,255,255,0.04)] h-[31.5px] relative rounded-[8.125px] shrink-0 w-full" data-name="ToolCallCard">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip p-px relative rounded-[inherit] size-full">
        <Button21 />
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8.125px]" />
    </div>
  );
}

function Container142() {
  return (
    <div className="h-[132.25px] relative shrink-0 w-[768.43px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col gap-[4.875px] items-start pt-[6.5px] relative size-full">
        <Container143 />
        <ToolCallCard7 />
        <ToolCallCard8 />
        <ToolCallCard9 />
      </div>
    </div>
  );
}

function Container139() {
  return (
    <div className="flex-[835.25_0_0] h-full min-w-px relative" data-name="Container">
      <div className="flex flex-col items-end size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-end pb-[9.75px] relative size-full">
          <Container140 />
          <Container141 />
          <Container142 />
        </div>
      </div>
    </div>
  );
}

function Icon57() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="Icon">
          <path d="M5.5 3.66667V1.83333H3.66667" id="Vector" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p15892640} id="Vector_2" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M0.916667 6.41667H1.83333" id="Vector_3" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M9.16667 6.41667H10.0833" id="Vector_4" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M6.875 5.95833V6.875" id="Vector_5" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d="M4.125 5.95833V6.875" id="Vector_6" stroke="var(--stroke-0, #409CFF)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
      </svg>
    </div>
  );
}

function Container149() {
  return (
    <div className="relative rounded-[16777200px] shrink-0 size-[24px]" style={{ backgroundImage: "linear-gradient(135deg, rgba(10, 132, 255, 0.4) 0%, rgba(94, 92, 230, 0.4) 100%)" }} data-name="Container">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.4)] border-solid inset-0 pointer-events-none rounded-[16777200px]" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center justify-center p-px relative size-full">
        <Icon57 />
      </div>
    </div>
  );
}

function Container148() {
  return (
    <div className="h-[224.969px] relative shrink-0 w-[24px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center pt-[3.25px] relative size-full">
        <Container149 />
      </div>
    </div>
  );
}

function MessageBubble6() {
  return (
    <div className="h-[231.469px] relative shrink-0 w-full" data-name="MessageBubble">
      <div aria-hidden className="absolute border-[rgba(0,0,0,0)] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row justify-end size-full">
        <div className="content-stretch flex gap-[9.75px] items-start justify-end pl-[13px] pr-[15px] py-[3.25px] relative size-full">
          <Container139 />
          <Container148 />
        </div>
      </div>
    </div>
  );
}

function MessageBubbleMargin2() {
  return (
    <div className="relative shrink-0 w-full" data-name="MessageBubble:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[8.125px] relative size-full">
        <MessageBubble6 />
      </div>
    </div>
  );
}

function Container135() {
  return (
    <div className="relative shrink-0 w-[897px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[8.125px] relative size-full">
        <Container136 />
        <MessageBubbleMargin2 />
      </div>
    </div>
  );
}

function TraceViewPanel3() {
  return (
    <div className="flex-[706_0_0] min-h-px relative w-[897px]" data-name="TraceViewPanel">
      <div className="overflow-clip rounded-[inherit] size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pb-[13px] pt-[6.5px] relative size-full">
          <MessageBubble />
          <Container76 />
          <MessageBubble2 />
          <MessageBubble3 />
          <MessageBubble4 />
          <Container126 />
          <Container135 />
        </div>
      </div>
    </div>
  );
}

function Container150() {
  return <div className="bg-[#30d158] relative rounded-[16777200px] shrink-0 size-[4.875px]" data-name="Container" />;
}

function Text111() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">7 of 7 messages</p>
      </div>
    </div>
  );
}

function Text112() {
  return (
    <div className="content-stretch flex flex-col items-start relative shrink-0" data-name="Text">
      <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">7,670 tokens total</p>
    </div>
  );
}

function TextAlign() {
  return (
    <div className="flex-[1_0_0] min-w-px relative" data-name="Text:align">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-start justify-end relative size-full">
        <Text112 />
      </div>
    </div>
  );
}

function TraceViewPanel4() {
  return (
    <div className="h-[28px] relative shrink-0 w-[897px]" data-name="TraceViewPanel">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[9.75px] items-center pt-px px-[13px] relative size-full">
        <Container150 />
        <Text111 />
        <TextAlign />
      </div>
    </div>
  );
}

function Container66() {
  return (
    <div className="flex-[898_0_0] h-full min-w-px relative" data-name="Container">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-r border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pr-px relative size-full">
        <PanelHeader1 />
        <FilterRow1 />
        <TraceViewPanel3 />
        <TraceViewPanel4 />
      </div>
    </div>
  );
}

function Text113() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#636366] text-[9.5px] whitespace-nowrap">A3F8B2D1</p>
      </div>
    </div>
  );
}

function Icon58() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g id="Icon">
          <path d={svgPaths.p1b3a0c40} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
        </g>
      </svg>
    </div>
  );
}

function Text114() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#636366] text-[9.5px] whitespace-nowrap">Turn 2</p>
      </div>
    </div>
  );
}

function Icon59() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g id="Icon">
          <path d={svgPaths.p1b3a0c40} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.75" />
        </g>
      </svg>
    </div>
  );
}

function Text115() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#409cff] text-[9.5px] whitespace-nowrap">assistant</p>
      </div>
    </div>
  );
}

function Container153() {
  return (
    <div className="relative shrink-0 w-full" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[3.25px] items-center relative size-full">
        <Text113 />
        <Icon58 />
        <Text114 />
        <Icon59 />
        <Text115 />
      </div>
    </div>
  );
}

function Text116() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[18px] not-italic relative shrink-0 text-[#e5e5ea] text-[12px] whitespace-nowrap">API Request</p>
      </div>
    </div>
  );
}

function Text117() {
  return (
    <div className="bg-[rgba(48,209,88,0.1)] relative rounded-[4px] shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start px-[6px] py-[2px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[14.25px] not-italic relative shrink-0 text-[#30d158] text-[9.5px] whitespace-nowrap">200</p>
      </div>
    </div>
  );
}

function Container154() {
  return (
    <div className="h-[20.625px] relative shrink-0 w-[162.563px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center pt-[1.625px] relative size-full">
        <Text116 />
        <Text117 />
      </div>
    </div>
  );
}

function EventDetailPanel() {
  return (
    <div className="relative shrink-0 w-[162.563px]" data-name="EventDetailPanel">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Container153 />
        <Container154 />
      </div>
    </div>
  );
}

function Container152() {
  return (
    <div className="flex-[210_0_0] min-w-px relative" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-center relative size-full">
        <EventDetailPanel />
      </div>
    </div>
  );
}

function Icon60() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_911)" id="Icon">
          <path d={svgPaths.p130b2500} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
          <path d={svgPaths.p26b52e00} id="Vector_2" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="0.916667" />
        </g>
        <defs>
          <clipPath id="clip0_1_911">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function Text118() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15.75px] not-italic relative shrink-0 text-[#636366] text-[10.5px] text-center whitespace-nowrap">Copy ID</p>
      </div>
    </div>
  );
}

function CopyButton1() {
  return (
    <div className="relative rounded-[8.125px] shrink-0" data-name="CopyButton">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[4.875px] items-center px-[8px] py-[4px] relative size-full">
        <Icon60 />
        <Text118 />
      </div>
    </div>
  );
}

function PanelHeader2() {
  return (
    <div className="h-[48px] relative shrink-0 w-full" data-name="PanelHeader">
      <div aria-hidden className="absolute border-[#0a84ff] border-b border-l-2 border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center pb-px pl-[12px] pr-[8px] relative size-full">
          <Container152 />
          <CopyButton1 />
        </div>
      </div>
    </div>
  );
}

function TextMargin() {
  return (
    <div className="relative shrink-0" data-name="Text:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[2px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[14.25px] not-italic relative shrink-0 text-[#636366] text-[9.5px] whitespace-nowrap">Method</p>
      </div>
    </div>
  );
}

function Container155() {
  return (
    <div className="col-1 justify-self-stretch relative row-1 self-stretch shrink-0" data-name="Container">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-r border-solid inset-0 pointer-events-none" />
      <div className="flex flex-col items-center justify-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center justify-center pl-[6px] pr-[7px] py-[10px] relative size-full">
          <p className="[word-break:break-word] font-['Menlo:Semi_Bold',sans-serif] leading-[18px] not-italic relative shrink-0 text-[#aeaeb2] text-[12px] whitespace-nowrap">POST</p>
          <TextMargin />
        </div>
      </div>
    </div>
  );
}

function TextMargin1() {
  return (
    <div className="relative shrink-0" data-name="Text:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[2px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[14.25px] not-italic relative shrink-0 text-[#636366] text-[9.5px] whitespace-nowrap">Duration</p>
      </div>
    </div>
  );
}

function Container156() {
  return (
    <div className="col-2 justify-self-stretch relative row-1 self-stretch shrink-0" data-name="Container">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-r border-solid inset-0 pointer-events-none" />
      <div className="flex flex-col items-center justify-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center justify-center pl-[6px] pr-[7px] py-[10px] relative size-full">
          <p className="[word-break:break-word] font-['Menlo:Semi_Bold',sans-serif] leading-[18px] not-italic relative shrink-0 text-[#aeaeb2] text-[12px] whitespace-nowrap">2.8s</p>
          <TextMargin1 />
        </div>
      </div>
    </div>
  );
}

function TextMargin2() {
  return (
    <div className="relative shrink-0" data-name="Text:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[2px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Regular',sans-serif] font-normal leading-[14.25px] not-italic relative shrink-0 text-[#636366] text-[9.5px] whitespace-nowrap">Tokens</p>
      </div>
    </div>
  );
}

function Container157() {
  return (
    <div className="col-3 justify-self-stretch relative row-1 self-stretch shrink-0" data-name="Container">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-r border-solid inset-0 pointer-events-none" />
      <div className="flex flex-col items-center justify-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-center justify-center pl-[6px] pr-[7px] py-[10px] relative size-full">
          <p className="[word-break:break-word] font-['Menlo:Semi_Bold',sans-serif] leading-[18px] not-italic relative shrink-0 text-[#aeaeb2] text-[12px] whitespace-nowrap">892</p>
          <TextMargin2 />
        </div>
      </div>
    </div>
  );
}

function EventDetailPanel1() {
  return (
    <div className="h-[55.25px] relative shrink-0 w-full" data-name="EventDetailPanel">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid grid grid-cols-[___102.66px_102.66px_102.66px] grid-rows-[_54.25px] pb-px relative size-full">
        <Container155 />
        <Container156 />
        <Container157 />
      </div>
    </div>
  );
}

function Container159() {
  return (
    <div className="relative shrink-0 w-full" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] tracking-[0.7px] uppercase whitespace-nowrap">Endpoint</p>
      </div>
    </div>
  );
}

function Container160() {
  return (
    <div className="content-stretch flex flex-col h-[15.75px] items-start overflow-clip relative shrink-0 w-full" data-name="Container">
      <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15.75px] not-italic relative shrink-0 text-[#aeaeb2] text-[10.5px] whitespace-nowrap">{`https://api.anthropic.com/v1/messages`}</p>
    </div>
  );
}

function ContainerMargin6() {
  return (
    <div className="relative shrink-0 w-full" data-name="Container:margin">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[4px] relative size-full">
        <Container160 />
      </div>
    </div>
  );
}

function Container161() {
  return (
    <div className="h-[17px] relative shrink-0 w-[284px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-[2px] relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Regular',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">req_01VcKjXm9aT3nPqLzYuFbHsR</p>
      </div>
    </div>
  );
}

function Container158() {
  return (
    <div className="relative shrink-0 w-full" data-name="Container">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pb-[11px] pt-[10px] px-[12px] relative size-full">
        <Container159 />
        <ContainerMargin6 />
        <Container161 />
      </div>
    </div>
  );
}

function Icon61() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M4.5 9L7.5 6L4.5 3" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text119() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon61 />
      </div>
    </div>
  );
}

function Text120() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[16.5px] not-italic relative shrink-0 text-[#8e8e93] text-[11px] whitespace-nowrap">Request Headers</p>
      </div>
    </div>
  );
}

function Text121() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">(4)</p>
      </div>
    </div>
  );
}

function Button22() {
  return (
    <div className="relative shrink-0 w-[308px]" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[12px] py-[8px] relative size-full">
        <Text119 />
        <Text120 />
        <Text121 />
      </div>
    </div>
  );
}

function CollapsibleSection() {
  return (
    <div className="relative shrink-0 w-full" data-name="CollapsibleSection">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-px relative size-full">
        <Button22 />
      </div>
    </div>
  );
}

function Icon62() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M3 4.5L6 7.5L9 4.5" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text122() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon62 />
      </div>
    </div>
  );
}

function Text123() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[16.5px] not-italic relative shrink-0 text-[#8e8e93] text-[11px] whitespace-nowrap">Request Body</p>
      </div>
    </div>
  );
}

function Button23() {
  return (
    <div className="relative shrink-0 w-full" data-name="Button">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[12px] py-[8px] relative size-full">
          <Text122 />
          <Text123 />
        </div>
      </div>
    </div>
  );
}

function JsonBlock() {
  return (
    <div className="h-[240px] max-h-[240px] relative shrink-0 w-full" data-name="JsonBlock">
      <div className="max-h-[inherit] overflow-clip rounded-[inherit] size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start max-h-[inherit] px-[12px] py-[8px] relative size-full">
          <div className="[word-break:break-word] font-['Cousine:Regular',sans-serif] leading-[0] not-italic relative shrink-0 text-[#3e3e4a] text-[0px] w-[879px] whitespace-pre-wrap">
            <p className="font-['Menlo:Regular',sans-serif] leading-[17.325px] mb-0 text-[10.5px]">{`{`}</p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"model"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"claude-opus-4-8"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"max_tokens"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#d49668]">8192</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"system"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"You are Claude Code, Anthropic's official CLI for Claude…"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"messages"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">[</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"role"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"user"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"content"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"Can you help me understand the current state of the authentication module and refactor it to use the new JWT middleware?"`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px]">],</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"tools"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">[</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"name"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"Read"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"description"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"Read a file from the filesystem"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"input_schema"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"object"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"properties"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"file_path"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`            `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"string"`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px]">{`},`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"required"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">[</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"file_path"`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px]">]</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`},`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"name"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"Glob"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"description"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"Find files matching a glob pattern"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"input_schema"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"object"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"properties"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"pattern"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`            `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"string"`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px]">{`},`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"required"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">[</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"pattern"`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px]">]</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`},`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"name"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"Grep"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"description"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"Search file contents with regex"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"input_schema"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"object"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"properties"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"pattern"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`            `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"string"`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`},`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"name"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"Edit"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"description"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"Edit a file"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"input_schema"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"object"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"properties"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"file_path"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`            `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"string"`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px]">{`},`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"old_string"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`            `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"string"`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px]">{`},`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"new_string"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`            `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"string"`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`},`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"name"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"Bash"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"description"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"Run a bash command"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"input_schema"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"object"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"properties"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"command"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`            `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"string"`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`          `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px]">]</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] leading-[17.325px] text-[10.5px]">{`}`}</p>
          </div>
        </div>
      </div>
    </div>
  );
}

function CollapsibleSection1() {
  return (
    <div className="relative shrink-0 w-full" data-name="CollapsibleSection">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-px relative size-full">
        <Button23 />
        <JsonBlock />
      </div>
    </div>
  );
}

function Icon63() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M4.5 9L7.5 6L4.5 3" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text124() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon63 />
      </div>
    </div>
  );
}

function Text125() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[16.5px] not-italic relative shrink-0 text-[#8e8e93] text-[11px] whitespace-nowrap">Response Headers</p>
      </div>
    </div>
  );
}

function Text126() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Menlo:Medium',sans-serif] leading-[15px] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">(4)</p>
      </div>
    </div>
  );
}

function Button24() {
  return (
    <div className="relative shrink-0 w-[308px]" data-name="Button">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[12px] py-[8px] relative size-full">
        <Text124 />
        <Text125 />
        <Text126 />
      </div>
    </div>
  );
}

function CollapsibleSection2() {
  return (
    <div className="relative shrink-0 w-full" data-name="CollapsibleSection">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-px relative size-full">
        <Button24 />
      </div>
    </div>
  );
}

function Icon64() {
  return (
    <div className="relative shrink-0 size-[12px]" data-name="Icon">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 12 12">
        <g id="Icon">
          <path d="M3 4.5L6 7.5L9 4.5" id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      </svg>
    </div>
  );
}

function Text127() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <Icon64 />
      </div>
    </div>
  );
}

function Text128() {
  return (
    <div className="relative shrink-0" data-name="Text">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[16.5px] not-italic relative shrink-0 text-[#8e8e93] text-[11px] whitespace-nowrap">Response Body</p>
      </div>
    </div>
  );
}

function Button25() {
  return (
    <div className="relative shrink-0 w-full" data-name="Button">
      <div className="flex flex-row items-center size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex gap-[6.5px] items-center px-[12px] py-[8px] relative size-full">
          <Text127 />
          <Text128 />
        </div>
      </div>
    </div>
  );
}

function JsonBlock1() {
  return (
    <div className="h-[280px] max-h-[280px] relative shrink-0 w-full" data-name="JsonBlock">
      <div className="max-h-[inherit] overflow-clip rounded-[inherit] size-full">
        <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start max-h-[inherit] px-[12px] py-[8px] relative size-full">
          <div className="[word-break:break-word] font-['Cousine:Regular',sans-serif] leading-[0] not-italic relative shrink-0 text-[#3e3e4a] text-[0px] w-[961px] whitespace-pre-wrap">
            <p className="font-['Menlo:Regular',sans-serif] leading-[17.325px] mb-0 text-[10.5px]">{`{`}</p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"id"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"msg_01VcKjXm9aT3nPqLzYuFbHsR"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"message"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"role"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"assistant"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"content"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">[</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"text"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"text"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"I'll start by reading the authentication module to understand its current structure, then look at the new JWT middleware implementation."`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`},`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"type"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"tool_use"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"id"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"tc1"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"name"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"Read"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"input"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`        `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"file_path"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"/src/auth/middleware.ts"`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`      `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px]">],</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"model"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"claude-opus-4-8"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"stop_reason"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#87bd78]">{`"tool_use"`}</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"usage"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px]">{`{`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"input_tokens"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#d49668]">892</span>
              <span className="leading-[17.325px]">,</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`    `}</span>
              <span className="leading-[17.325px] text-[#79b8d4]">{`"output_tokens"`}</span>
              <span className="leading-[17.325px]">:</span>
              <span className="leading-[17.325px] text-[#e5e5ea]">{` `}</span>
              <span className="leading-[17.325px] text-[#d49668]">187</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] mb-0 text-[10.5px]">
              <span className="leading-[17.325px] text-[#e5e5ea]">{`  `}</span>
              <span className="leading-[17.325px]">{`}`}</span>
            </p>
            <p className="font-['Menlo:Regular',sans-serif] leading-[17.325px] text-[10.5px]">{`}`}</p>
          </div>
        </div>
      </div>
    </div>
  );
}

function CollapsibleSection3() {
  return (
    <div className="relative shrink-0 w-full" data-name="CollapsibleSection">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start pt-px relative size-full">
        <Button25 />
        <JsonBlock1 />
      </div>
    </div>
  );
}

function EventDetailPanel2() {
  return (
    <div className="flex-[718.75_0_0] min-h-px relative w-full" data-name="EventDetailPanel">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <Container158 />
        <CollapsibleSection />
        <CollapsibleSection1 />
        <CollapsibleSection2 />
        <CollapsibleSection3 />
      </div>
    </div>
  );
}

function Container151() {
  return (
    <div className="h-full relative shrink-0 w-[308px]" data-name="Container">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex flex-col items-start relative size-full">
        <PanelHeader2 />
        <EventDetailPanel1 />
        <EventDetailPanel2 />
      </div>
    </div>
  );
}

function App() {
  return (
    <div className="bg-[#1c1c1e] h-[822px] relative shrink-0 w-[1551px]" data-name="App">
      <div className="bg-clip-padding border-0 border-[transparent] border-solid content-stretch flex items-start overflow-clip relative rounded-[inherit] size-full">
        <Container />
        <Container64 />
        <Container66 />
        <Container151 />
      </div>
    </div>
  );
}

export default function Document() {
  return (
    <div className="bg-[#1c1c1e] content-stretch flex flex-col items-start relative size-full" data-name="Document">
      <App />
    </div>
  );
}