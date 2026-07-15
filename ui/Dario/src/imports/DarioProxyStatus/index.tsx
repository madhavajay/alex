import svgPaths from "./svg-42ls8ze1sd";

function CounterBadge() {
  return (
    <div className="bg-[rgba(255,255,255,0.06)] content-stretch flex items-start px-[6px] py-[2px] relative rounded-[4px] shrink-0" data-name="CounterBadge">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#8e8e93] text-[10px] whitespace-nowrap">10</p>
    </div>
  );
}

function Container1() {
  return (
    <div className="content-stretch flex flex-[1_0_0] gap-[6.5px] items-center min-w-px relative" data-name="Container">
      <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[normal] not-italic relative shrink-0 text-[#e5e5ea] text-[13px] whitespace-nowrap">Sessions</p>
      <CounterBadge />
    </div>
  );
}

function PanelHeader() {
  return (
    <div className="h-[48px] relative shrink-0 w-full" data-name="PanelHeader">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="content-stretch flex items-center pl-[12px] pr-[8px] relative size-full">
          <Container1 />
        </div>
      </div>
    </div>
  );
}

function Search() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="search">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g clipPath="url(#clip0_1_209)" id="search">
          <path d={svgPaths.pa33b280} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" />
        </g>
        <defs>
          <clipPath id="clip0_1_209">
            <rect fill="white" height="11" width="11" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function SearchInput() {
  return (
    <div className="bg-[rgba(255,255,255,0.06)] flex-[1_0_0] h-[28px] min-w-px relative rounded-[8px]" data-name="SearchInput">
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8px]" />
      <div className="flex flex-row items-center size-full">
        <div className="content-stretch flex gap-[6.5px] items-center px-[8px] relative size-full">
          <Search />
          <p className="[word-break:break-word] flex-[1_0_0] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] min-w-px relative text-[11px] text-[rgba(229,229,234,0.5)]">Search sessions...</p>
        </div>
      </div>
    </div>
  );
}

function FilterRow() {
  return (
    <div className="bg-[#1c1c1e] h-[40px] relative shrink-0 w-full" data-name="FilterRow">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="content-stretch flex items-center px-[10px] relative size-full">
          <SearchInput />
        </div>
      </div>
    </div>
  );
}

function SessionListHeader() {
  return (
    <div className="h-[24px] relative shrink-0 w-full" data-name="SessionListHeader">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="[word-break:break-word] grid grid-cols-[_____96.77px_2.89px_0.32px_0.01px_0.01px] grid-rows-[_100px] leading-[normal] not-italic pl-[20px] pr-[8px] relative size-full text-[#636366] text-[10px] whitespace-nowrap">
        <p className="col-1 font-['Inter:Medium',sans-serif] font-medium justify-self-start relative row-1 self-start shrink-0">Session</p>
        <p className="col-2 font-['Inter:Regular',sans-serif] font-normal justify-self-start relative row-1 self-start shrink-0 text-right">T</p>
        <p className="col-3 font-['Inter:Regular',sans-serif] font-normal justify-self-start relative row-1 self-start shrink-0 text-right">Cost</p>
        <p className="col-4 font-['Inter:Regular',sans-serif] font-normal justify-self-start relative row-1 self-start shrink-0 text-right">Dur</p>
        <p className="col-5 font-['Inter:Regular',sans-serif] font-normal justify-self-start relative row-1 self-start shrink-0 text-right">Account</p>
      </div>
    </div>
  );
}

function Button() {
  return (
    <div className="bg-[rgba(255,255,255,0.1)] content-stretch flex flex-col items-center justify-center px-[10px] py-[4px] relative rounded-[6px] shrink-0" data-name="Button">
      <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[normal] not-italic relative shrink-0 text-[#e5e5ea] text-[10px] whitespace-nowrap">All</p>
    </div>
  );
}

function Button1() {
  return (
    <div className="content-stretch flex flex-col items-center justify-center px-[10px] py-[4px] relative rounded-[6px] shrink-0" data-name="Button">
      <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[normal] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">Running</p>
    </div>
  );
}

function Button2() {
  return (
    <div className="content-stretch flex flex-col items-center justify-center px-[10px] py-[4px] relative rounded-[6px] shrink-0" data-name="Button">
      <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[normal] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">Error</p>
    </div>
  );
}

function Button3() {
  return (
    <div className="content-stretch flex flex-col items-center justify-center px-[10px] py-[4px] relative rounded-[6px] shrink-0" data-name="Button">
      <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[normal] not-italic relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">Done</p>
    </div>
  );
}

function FilterTabBar() {
  return (
    <div className="h-[32px] relative shrink-0 w-full" data-name="FilterTabBar">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="content-stretch flex gap-[5px] items-center px-[10px] relative size-full">
          <Button />
          <Button1 />
          <Button2 />
          <Button3 />
        </div>
      </div>
    </div>
  );
}

function ChevronRight() {
  return (
    <div className="relative shrink-0 size-[11px]" data-name="chevron-right">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 11 11">
        <g id="chevron-right">
          <path d={svgPaths.p1a78e480} id="Vector" stroke="var(--stroke-0, #636366)" strokeLinecap="round" strokeWidth="2" />
        </g>
      </svg>
    </div>
  );
}

function Terminal() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="terminal">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g clipPath="url(#clip0_1_196)" id="terminal">
          <path d={svgPaths.p3128be40} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeWidth="2" />
        </g>
        <defs>
          <clipPath id="clip0_1_196">
            <rect fill="white" height="9" width="9" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function Harness() {
  return (
    <div className="bg-[rgba(10,132,255,0.18)] content-stretch flex items-center justify-center relative rounded-[3px] shrink-0 size-[17px]" data-name="Harness">
      <Terminal />
    </div>
  );
}

function Provider() {
  return (
    <div className="bg-[rgba(255,144,64,0.18)] content-stretch flex items-center justify-center relative rounded-[3px] shrink-0 size-[17px]" data-name="Provider">
      <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[normal] not-italic relative shrink-0 text-[#ff9040] text-[9px] whitespace-nowrap">A</p>
    </div>
  );
}

function Model() {
  return (
    <div className="bg-[rgba(191,90,242,0.12)] content-stretch flex items-start px-[6px] py-[2px] relative rounded-[5px] shrink-0" data-name="Model">
      <div aria-hidden className="absolute border border-[rgba(191,90,242,0.25)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <p className="[word-break:break-word] font-['JetBrains_Mono:Medium',sans-serif] font-medium leading-[normal] relative shrink-0 text-[#bf5af2] text-[9.5px] whitespace-nowrap">opus 4.8</p>
    </div>
  );
}

function Cols() {
  return (
    <div className="col-1 content-stretch flex gap-[5px] items-center justify-self-start relative row-1 self-start shrink-0" data-name="Cols">
      <ChevronRight />
      <div className="relative shrink-0 size-[5px]" data-name="Ellipse">
        <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 5 5">
          <circle cx="2.5" cy="2.5" fill="var(--fill-0, #30D158)" id="Ellipse" r="2.5" />
        </svg>
      </div>
      <Harness />
      <Provider />
      <Model />
    </div>
  );
}

function ActiveRow() {
  return (
    <div className="bg-[rgba(10,132,255,0.07)] h-[30px] relative shrink-0 w-full" data-name="ActiveRow">
      <div aria-hidden className="absolute border-[#0a84ff] border-r-2 border-solid inset-0 pointer-events-none" />
      <div className="grid grid-cols-[_____97.10px_2.60px_0.29px_0.01px_0.01px] grid-rows-[_100px] pl-[4px] pr-[8px] relative size-full">
        <Cols />
        <div className="[word-break:break-word] col-2 flex flex-col font-['JetBrains_Mono:Regular',sans-serif] font-normal justify-center justify-self-start leading-[0] relative row-1 self-start shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">
          <p className="leading-[normal]">7</p>
        </div>
        <div className="[word-break:break-word] col-3 flex flex-col font-['JetBrains_Mono:Regular',sans-serif] font-normal justify-center justify-self-start leading-[0] relative row-1 self-start shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">
          <p className="leading-[normal]">$0.84</p>
        </div>
        <div className="[word-break:break-word] col-4 flex flex-col font-['JetBrains_Mono:Regular',sans-serif] font-normal justify-center justify-self-start leading-[0] relative row-1 self-start shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">
          <p className="leading-[normal]">42.1s</p>
        </div>
        <div className="[word-break:break-word] col-5 flex flex-col font-['Inter:Regular',sans-serif] font-normal justify-center justify-self-start leading-[0] not-italic overflow-hidden relative row-1 self-start shrink-0 text-[#636366] text-[10px] text-ellipsis text-right whitespace-nowrap">
          <p className="leading-[normal] overflow-hidden text-ellipsis">prod-api</p>
        </div>
      </div>
    </div>
  );
}

function BranchLine() {
  return (
    <div className="content-stretch flex items-center pl-[8px] relative shrink-0 w-[24px]" data-name="BranchLine">
      <div className="bg-[#636366] h-[12px] relative shrink-0 w-px" data-name="Rectangle" />
      <div className="bg-[#636366] h-px relative shrink-0 w-[8px]" data-name="Rectangle" />
    </div>
  );
}

function Terminal1() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="terminal">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g clipPath="url(#clip0_1_196)" id="terminal">
          <path d={svgPaths.p3128be40} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeWidth="2" />
        </g>
        <defs>
          <clipPath id="clip0_1_196">
            <rect fill="white" height="9" width="9" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function Harness1() {
  return (
    <div className="bg-[rgba(10,132,255,0.18)] content-stretch flex items-center justify-center relative rounded-[3px] shrink-0 size-[17px]" data-name="Harness">
      <Terminal1 />
    </div>
  );
}

function Provider1() {
  return (
    <div className="bg-[rgba(255,144,64,0.18)] content-stretch flex items-center justify-center relative rounded-[3px] shrink-0 size-[17px]" data-name="Provider">
      <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[normal] not-italic relative shrink-0 text-[#ff9040] text-[9px] whitespace-nowrap">A</p>
    </div>
  );
}

function Model1() {
  return (
    <div className="bg-[rgba(10,132,255,0.12)] content-stretch flex items-start px-[6px] py-[2px] relative rounded-[5px] shrink-0" data-name="Model">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.25)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <p className="[word-break:break-word] font-['JetBrains_Mono:Medium',sans-serif] font-medium leading-[normal] relative shrink-0 text-[#0a84ff] text-[9.5px] whitespace-nowrap">sonnet 4.6</p>
    </div>
  );
}

function Cols1() {
  return (
    <div className="col-1 content-stretch flex gap-[5px] items-center justify-self-start relative row-1 self-start shrink-0" data-name="Cols">
      <BranchLine />
      <div className="relative shrink-0 size-[5px]" data-name="Ellipse">
        <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 5 5">
          <circle cx="2.5" cy="2.5" fill="var(--fill-0, #30D158)" id="Ellipse" r="2.5" />
        </svg>
      </div>
      <Harness1 />
      <Provider1 />
      <Model1 />
    </div>
  );
}

function SessionRow() {
  return (
    <div className="h-[30px] relative shrink-0 w-full" data-name="SessionRow">
      <div className="grid grid-cols-[_____97.18px_2.53px_0.28px_0.01px_0.01px] grid-rows-[_100px] pr-[8px] relative size-full">
        <Cols1 />
        <div className="[word-break:break-word] col-2 flex flex-col font-['JetBrains_Mono:Regular',sans-serif] font-normal justify-center justify-self-start leading-[0] relative row-1 self-start shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">
          <p className="leading-[normal]">6</p>
        </div>
        <div className="[word-break:break-word] col-3 flex flex-col font-['JetBrains_Mono:Regular',sans-serif] font-normal justify-center justify-self-start leading-[0] relative row-1 self-start shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">
          <p className="leading-[normal]">$0.18</p>
        </div>
        <div className="[word-break:break-word] col-4 flex flex-col font-['JetBrains_Mono:Regular',sans-serif] font-normal justify-center justify-self-start leading-[0] relative row-1 self-start shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">
          <p className="leading-[normal]">8.4s</p>
        </div>
        <div className="[word-break:break-word] col-5 flex flex-col font-['Inter:Regular',sans-serif] font-normal justify-center justify-self-start leading-[0] not-italic overflow-hidden relative row-1 self-start shrink-0 text-[#636366] text-[10px] text-ellipsis text-right whitespace-nowrap">
          <p className="leading-[normal] overflow-hidden text-ellipsis">prod-api</p>
        </div>
      </div>
    </div>
  );
}

function Terminal2() {
  return (
    <div className="relative shrink-0 size-[9px]" data-name="terminal">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 9 9">
        <g clipPath="url(#clip0_1_196)" id="terminal">
          <path d={svgPaths.p3128be40} id="Vector" stroke="var(--stroke-0, #AEAEB2)" strokeLinecap="round" strokeWidth="2" />
        </g>
        <defs>
          <clipPath id="clip0_1_196">
            <rect fill="white" height="9" width="9" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function Harness2() {
  return (
    <div className="bg-[rgba(191,90,242,0.15)] content-stretch flex items-center justify-center relative rounded-[3px] shrink-0 size-[17px]" data-name="Harness">
      <Terminal2 />
    </div>
  );
}

function Provider2() {
  return (
    <div className="bg-[rgba(255,144,64,0.18)] content-stretch flex items-center justify-center relative rounded-[3px] shrink-0 size-[17px]" data-name="Provider">
      <p className="[word-break:break-word] font-['Inter:Bold',sans-serif] font-bold leading-[normal] not-italic relative shrink-0 text-[#ff9040] text-[9px] whitespace-nowrap">A</p>
    </div>
  );
}

function Model2() {
  return (
    <div className="bg-[rgba(10,132,255,0.12)] content-stretch flex items-start px-[6px] py-[2px] relative rounded-[5px] shrink-0" data-name="Model">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.25)] border-solid inset-0 pointer-events-none rounded-[5px]" />
      <p className="[word-break:break-word] font-['JetBrains_Mono:Medium',sans-serif] font-medium leading-[normal] relative shrink-0 text-[#0a84ff] text-[9.5px] whitespace-nowrap">sonnet 4.6</p>
    </div>
  );
}

function Cols2() {
  return (
    <div className="col-1 content-stretch flex gap-[5px] items-center justify-self-start relative row-1 self-start shrink-0" data-name="Cols">
      <div className="relative shrink-0 size-[5px]" data-name="Ellipse">
        <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 5 5">
          <circle cx="2.5" cy="2.5" fill="var(--fill-0, #FF453A)" id="Ellipse" r="2.5" />
        </svg>
      </div>
      <Harness2 />
      <Provider2 />
      <Model2 />
    </div>
  );
}

function SessionRow1() {
  return (
    <div className="h-[30px] relative shrink-0 w-full" data-name="SessionRow">
      <div className="grid grid-cols-[_____97.10px_2.60px_0.29px_0.01px_0.01px] grid-rows-[_100px] pl-[4px] pr-[8px] relative size-full">
        <Cols2 />
        <div className="[word-break:break-word] col-2 flex flex-col font-['JetBrains_Mono:Regular',sans-serif] font-normal justify-center justify-self-start leading-[0] relative row-1 self-start shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">
          <p className="leading-[normal]">3</p>
        </div>
        <div className="[word-break:break-word] col-3 flex flex-col font-['JetBrains_Mono:Regular',sans-serif] font-normal justify-center justify-self-start leading-[0] relative row-1 self-start shrink-0 text-[#636366] text-[10.5px] text-right whitespace-nowrap">
          <p className="leading-[normal]">$0.12</p>
        </div>
        <div className="[word-break:break-word] col-4 flex flex-col font-['JetBrains_Mono:Regular',sans-serif] font-normal justify-center justify-self-start leading-[0] relative row-1 self-start shrink-0 text-[#636366] text-[10px] text-right whitespace-nowrap">
          <p className="leading-[normal]">8.9s</p>
        </div>
        <div className="[word-break:break-word] col-5 flex flex-col font-['Inter:Regular',sans-serif] font-normal justify-center justify-self-start leading-[0] not-italic overflow-hidden relative row-1 self-start shrink-0 text-[#636366] text-[10px] text-ellipsis text-right whitespace-nowrap">
          <p className="leading-[normal] overflow-hidden text-ellipsis">staging</p>
        </div>
      </div>
    </div>
  );
}

function SessionList() {
  return (
    <div className="content-stretch flex flex-[1_0_0] flex-col items-start min-h-px overflow-clip relative w-full" data-name="SessionList">
      <ActiveRow />
      <SessionRow />
      <SessionRow1 />
    </div>
  );
}

function SessionListFooter() {
  return (
    <div className="h-[28px] relative shrink-0 w-full" data-name="SessionListFooter">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="content-stretch flex items-center px-[12px] relative size-full">
          <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">8 of 10 sessions</p>
        </div>
      </div>
    </div>
  );
}

function Container() {
  return (
    <div className="content-stretch flex flex-col h-full items-start relative shrink-0 w-[340px]" data-name="Container">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-r border-solid inset-0 pointer-events-none" />
      <PanelHeader />
      <FilterRow />
      <SessionListHeader />
      <FilterTabBar />
      <SessionList />
      <SessionListFooter />
    </div>
  );
}

function Cpu() {
  return (
    <div className="relative shrink-0 size-[14px]" data-name="cpu">
      <svg className="absolute block inset-0 size-full" fill="none" preserveAspectRatio="none" viewBox="0 0 14 14">
        <g clipPath="url(#clip0_1_206)" id="cpu">
          <path d={svgPaths.p188e8800} id="Vector" stroke="var(--stroke-0, #0A84FF)" strokeLinecap="round" strokeWidth="2" />
        </g>
        <defs>
          <clipPath id="clip0_1_206">
            <rect fill="white" height="14" width="14" />
          </clipPath>
        </defs>
      </svg>
    </div>
  );
}

function LogoBox() {
  return (
    <div className="bg-[rgba(10,132,255,0.15)] content-stretch flex items-center justify-center relative rounded-[8px] shrink-0 size-[30px]" data-name="LogoBox">
      <div aria-hidden className="absolute border border-[rgba(10,132,255,0.22)] border-solid inset-0 pointer-events-none rounded-[8px]" />
      <Cpu />
    </div>
  );
}

function Titles() {
  return (
    <div className="[word-break:break-word] content-stretch flex flex-col items-start leading-[normal] not-italic relative shrink-0 whitespace-nowrap" data-name="Titles">
      <p className="font-['Inter:Semi_Bold',sans-serif] font-semibold relative shrink-0 text-[#e5e5ea] text-[14px]">Alexandria - Dario</p>
      <p className="font-['Inter:Regular',sans-serif] font-normal relative shrink-0 text-[#636366] text-[10px]">Dario 5.1.1 - active gen-5.1.1-50932</p>
    </div>
  );
}

function HeaderLeft() {
  return (
    <div className="content-stretch flex gap-[12px] items-center relative shrink-0" data-name="HeaderLeft">
      <LogoBox />
      <Titles />
    </div>
  );
}

function HeaderButton() {
  return (
    <div className="bg-[rgba(255,255,255,0.06)] content-stretch flex flex-col items-center justify-center px-[12px] py-[6px] relative rounded-[6px] shrink-0" data-name="HeaderButton">
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[6px]" />
      <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[normal] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">Restart</p>
    </div>
  );
}

function HeaderButton1() {
  return (
    <div className="bg-[rgba(255,255,255,0.06)] content-stretch flex flex-col items-center justify-center px-[12px] py-[6px] relative rounded-[6px] shrink-0" data-name="HeaderButton">
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[6px]" />
      <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[normal] not-italic relative shrink-0 text-[#e5e5ea] text-[11px] whitespace-nowrap">Check Update</p>
    </div>
  );
}

function HeaderRight() {
  return (
    <div className="content-stretch flex gap-[8px] items-center relative shrink-0" data-name="HeaderRight">
      <HeaderButton />
      <HeaderButton1 />
    </div>
  );
}

function Header() {
  return (
    <div className="h-[48px] relative shrink-0 w-full" data-name="Header">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="content-stretch flex items-center justify-between px-[20px] relative size-full">
          <HeaderLeft />
          <HeaderRight />
        </div>
      </div>
    </div>
  );
}

function SubtitleHelp() {
  return (
    <div className="bg-[#141414] relative shrink-0 w-full" data-name="SubtitleHelp">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-b border-solid inset-0 pointer-events-none" />
      <div className="content-stretch flex items-start px-[20px] py-[8px] relative size-full">
        <p className="[word-break:break-word] flex-[1_0_0] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[0] min-w-px relative text-[#636366] text-[10.5px]">
          <span className="leading-[normal]">{`Logs path: /generative-health.html logs. Dario-routed traffic shows up in the Trace Browser under account `}</span>
          <span className="leading-[normal] text-[#0a84ff]">{`demo:<generation>`}</span>
        </p>
      </div>
    </div>
  );
}

function GenHeader() {
  return (
    <div className="bg-[#141414] h-[28px] relative shrink-0 w-full" data-name="GenHeader">
      <div className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold grid grid-cols-[________96.92px_2.87px_0.20px_0.01px_0.01px_0px_0px_0px] grid-rows-[_100px] leading-[0] not-italic px-[16px] relative size-full text-[#636366] text-[10.5px] whitespace-nowrap">
        <div className="col-1 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0">
          <p className="leading-[normal]">generation</p>
        </div>
        <div className="col-2 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0">
          <p className="leading-[normal]">version</p>
        </div>
        <div className="col-3 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0">
          <p className="leading-[normal]">phase</p>
        </div>
        <div className="col-4 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0">
          <p className="leading-[normal]">port</p>
        </div>
        <div className="col-5 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0">
          <p className="leading-[normal]">pid</p>
        </div>
        <div className="col-6 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0">
          <p className="leading-[normal]">busy</p>
        </div>
        <div className="col-7 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0">
          <p className="leading-[normal]">probe</p>
        </div>
        <div className="col-8 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0 text-right">
          <p className="leading-[normal]">age</p>
        </div>
      </div>
    </div>
  );
}

function CellGeneration() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[140px]" data-name="cell-generation">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Bold',sans-serif] font-bold leading-[normal] relative shrink-0 text-[#0a84ff] text-[11px] w-full">gen-5.1.1-50932</p>
    </div>
  );
}

function CellVersion() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[60px]" data-name="cell-version">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#e5e5ea] text-[11px] w-full">5.1.1</p>
    </div>
  );
}

function CellPhase() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[80px]" data-name="cell-phase">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Bold',sans-serif] font-bold leading-[normal] relative shrink-0 text-[#30d158] text-[11px] w-full">ready</p>
    </div>
  );
}

function CellPort() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[60px]" data-name="cell-port">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#e5e5ea] text-[11px] w-full">50932</p>
    </div>
  );
}

function CellPid() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[60px]" data-name="cell-pid">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#e5e5ea] text-[11px] w-full">68113</p>
    </div>
  );
}

function CellBusy() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[40px]" data-name="cell-busy">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#e5e5ea] text-[11px] w-full">0</p>
    </div>
  );
}

function CellProbe() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[60px]" data-name="cell-probe">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#636366] text-[11px] w-full">-</p>
    </div>
  );
}

function CellAge() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[60px]" data-name="cell-age">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#e5e5ea] text-[11px] text-right w-full">59s</p>
    </div>
  );
}

function GenRow() {
  return (
    <div className="bg-[rgba(10,132,255,0.07)] h-[36px] relative shrink-0 w-full" data-name="GenRow">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="content-stretch flex items-center px-[16px] relative size-full">
          <CellGeneration />
          <CellVersion />
          <CellPhase />
          <CellPort />
          <CellPid />
          <CellBusy />
          <CellProbe />
          <CellAge />
        </div>
      </div>
    </div>
  );
}

function TableContainer() {
  return (
    <div className="relative rounded-[8px] shrink-0 w-full" data-name="TableContainer">
      <div className="content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <GenHeader />
        <GenRow />
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8px]" />
    </div>
  );
}

function GenerationSection() {
  return (
    <div className="relative shrink-0 w-full" data-name="GenerationSection">
      <div className="content-stretch flex flex-col gap-[8px] items-start p-[16px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[normal] not-italic relative shrink-0 text-[#8e8e93] text-[11px] uppercase whitespace-nowrap">GENERATION</p>
        <TableContainer />
      </div>
    </div>
  );
}

function CacheHeader() {
  return (
    <div className="bg-[#141414] h-[28px] relative shrink-0 w-full" data-name="CacheHeader">
      <div className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold grid grid-cols-[______98.88px_1.09px_0.04px_0.01px_0px_0px] grid-rows-[_100px] leading-[0] not-italic px-[16px] relative size-full text-[#636366] text-[10.5px] whitespace-nowrap">
        <div className="col-1 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0">
          <p className="leading-[normal]">cache</p>
        </div>
        <div className="col-2 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0">
          <p className="leading-[normal]">status</p>
        </div>
        <div className="col-3 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0">
          <p className="leading-[normal]">chars</p>
        </div>
        <div className="col-4 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0">
          <p className="leading-[normal]">version</p>
        </div>
        <div className="col-5 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0">
          <p className="leading-[normal]">last used</p>
        </div>
        <div className="col-6 flex flex-col justify-center justify-self-start relative row-1 self-start shrink-0 text-right">
          <p className="leading-[normal]">action</p>
        </div>
      </div>
    </div>
  );
}

function CellName() {
  return (
    <div className="[word-break:break-word] content-stretch flex flex-col gap-[2px] items-start justify-center leading-[normal] relative shrink-0 w-[260px]" data-name="cell-name">
      <p className="font-['JetBrains_Mono:Bold',sans-serif] font-bold relative shrink-0 text-[#e5e5ea] text-[11px] w-full">claude-haiku-4-5</p>
      <p className="font-['JetBrains_Mono:Regular',sans-serif] font-normal overflow-hidden relative shrink-0 text-[#636366] text-[9.5px] text-ellipsis w-full whitespace-nowrap">/Users/mochav/dev/.alexandria/dario-prompt-cache/claude-haiku-4-5-f73a3fface2eb.json</p>
    </div>
  );
}

function CellStatus() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[60px]" data-name="cell-status">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#30d158] text-[11px] w-full">hit</p>
    </div>
  );
}

function CellChars() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[60px]" data-name="cell-chars">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#e5e5ea] text-[11px] w-full">26941</p>
    </div>
  );
}

function CellVersion1() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[70px]" data-name="cell-version">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#e5e5ea] text-[11px] w-full">2.1.207</p>
    </div>
  );
}

function CellLastUsed() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[80px]" data-name="cell-last-used">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#636366] text-[11px] w-full">-</p>
    </div>
  );
}

function Frame() {
  return (
    <div className="bg-[rgba(255,255,255,0.06)] content-stretch flex items-start px-[8px] py-[3px] relative rounded-[4px] shrink-0" data-name="Frame">
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[4px]" />
      <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[normal] not-italic relative shrink-0 text-[#e5e5ea] text-[9.5px] whitespace-nowrap">Clear</p>
    </div>
  );
}

function CellAction() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[120px]" data-name="cell-action">
      <Frame />
    </div>
  );
}

function CacheRow() {
  return (
    <div className="h-[48px] relative shrink-0 w-full" data-name="CacheRow1">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="content-stretch flex items-center px-[16px] relative size-full">
          <CellName />
          <CellStatus />
          <CellChars />
          <CellVersion1 />
          <CellLastUsed />
          <CellAction />
        </div>
      </div>
    </div>
  );
}

function CellName1() {
  return (
    <div className="[word-break:break-word] content-stretch flex flex-col gap-[2px] items-start justify-center leading-[normal] relative shrink-0 w-[260px]" data-name="cell-name">
      <p className="font-['JetBrains_Mono:Bold',sans-serif] font-bold relative shrink-0 text-[#e5e5ea] text-[11px] w-full">claude-opus-4-8</p>
      <p className="font-['JetBrains_Mono:Regular',sans-serif] font-normal overflow-hidden relative shrink-0 text-[#636366] text-[9.5px] text-ellipsis w-full whitespace-nowrap">/Users/mochav/dev/.alexandria/dario-prompt-cache/claude-opus-4-8-efaeac877ff7.json</p>
    </div>
  );
}

function CellStatus1() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[60px]" data-name="cell-status">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#30d158] text-[11px] w-full">hit</p>
    </div>
  );
}

function CellChars1() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[60px]" data-name="cell-chars">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#e5e5ea] text-[11px] w-full">5440</p>
    </div>
  );
}

function CellVersion2() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[70px]" data-name="cell-version">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#e5e5ea] text-[11px] w-full">2.1.207</p>
    </div>
  );
}

function CellLastUsed1() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[80px]" data-name="cell-last-used">
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#636366] text-[11px] w-full">-</p>
    </div>
  );
}

function Frame1() {
  return (
    <div className="bg-[rgba(255,255,255,0.06)] content-stretch flex items-start px-[8px] py-[3px] relative rounded-[4px] shrink-0" data-name="Frame">
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[4px]" />
      <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[normal] not-italic relative shrink-0 text-[#e5e5ea] text-[9.5px] whitespace-nowrap">Clear</p>
    </div>
  );
}

function CellAction1() {
  return (
    <div className="content-stretch flex flex-col items-start justify-center relative shrink-0 w-[120px]" data-name="cell-action">
      <Frame1 />
    </div>
  );
}

function CacheRow1() {
  return (
    <div className="h-[48px] relative shrink-0 w-full" data-name="CacheRow2">
      <div aria-hidden className="absolute border-[rgba(255,255,255,0.07)] border-solid border-t inset-0 pointer-events-none" />
      <div className="flex flex-row items-center size-full">
        <div className="content-stretch flex items-center px-[16px] relative size-full">
          <CellName1 />
          <CellStatus1 />
          <CellChars1 />
          <CellVersion2 />
          <CellLastUsed1 />
          <CellAction1 />
        </div>
      </div>
    </div>
  );
}

function TableContainer1() {
  return (
    <div className="relative rounded-[8px] shrink-0 w-full" data-name="TableContainer">
      <div className="content-stretch flex flex-col items-start overflow-clip relative rounded-[inherit] size-full">
        <CacheHeader />
        <CacheRow />
        <CacheRow1 />
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8px]" />
    </div>
  );
}

function PromptCacheSection() {
  return (
    <div className="relative shrink-0 w-full" data-name="PromptCacheSection">
      <div className="content-stretch flex flex-col gap-[8px] items-start pb-[16px] px-[16px] relative size-full">
        <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[normal] not-italic relative shrink-0 text-[#8e8e93] text-[11px] uppercase whitespace-nowrap">PROMPT CACHE</p>
        <TableContainer1 />
      </div>
    </div>
  );
}

function TabStdout() {
  return (
    <div className="bg-[#0a84ff] content-stretch flex items-start px-[10px] py-[4px] relative rounded-[6px] shrink-0" data-name="TabStdout">
      <p className="[word-break:break-word] font-['Inter:Semi_Bold',sans-serif] font-semibold leading-[normal] not-italic relative shrink-0 text-[#e5e5ea] text-[10.5px] whitespace-nowrap">stdout</p>
    </div>
  );
}

function TabStderr() {
  return (
    <div className="bg-[rgba(255,255,255,0.06)] content-stretch flex items-start px-[10px] py-[4px] relative rounded-[6px] shrink-0" data-name="TabStderr">
      <p className="[word-break:break-word] font-['Inter:Medium',sans-serif] font-medium leading-[normal] not-italic relative shrink-0 text-[#8e8e93] text-[10.5px] whitespace-nowrap">stderr</p>
    </div>
  );
}

function TabBar() {
  return (
    <div className="content-stretch flex gap-[8px] h-[28px] items-center relative shrink-0 w-full" data-name="TabBar">
      <TabStdout />
      <TabStderr />
      <p className="[word-break:break-word] font-['JetBrains_Mono:Regular',sans-serif] font-normal leading-[normal] relative shrink-0 text-[#636366] text-[10px] whitespace-nowrap">gen-5.1.1-50932</p>
    </div>
  );
}

function ConsoleOutput() {
  return (
    <div className="bg-[#111112] flex-[1_0_0] min-h-px relative rounded-[8px] w-full" data-name="ConsoleOutput">
      <div className="overflow-clip rounded-[inherit] size-full">
        <div className="[word-break:break-word] content-stretch flex flex-col font-['JetBrains_Mono:Regular',sans-serif] font-normal gap-[4px] items-start leading-[15.5px] p-[12px] relative size-full text-[10.5px]">
          <p className="relative shrink-0 text-[#e5e5ea] w-full">Device identity: detected</p>
          <p className="relative shrink-0 text-[#8e8e93] w-full">dario | template: live capture, DE v2.1.210 (2h old)</p>
          <p className="relative shrink-0 text-[#8e8e93] w-full">{`dario | * DE compat: installed DE v2.1.210 is newer than dario's last tested version (v2.1.209): usually fine, but untested`}</p>
          <p className="relative shrink-0 text-[#8e8e93] w-full">dario | * TLS Fingerprint: Node v23.6.1 - Bun v1.3.14 on PATH but auto-relaunch bypassed (DARIO_NO_RUN)</p>
          <p className="relative shrink-0 text-[#8e8e93] w-full">dario | + unset DARIO_NO_RUN to auto-relaunch under Bun on the next invocation.</p>
          <p className="relative shrink-0 text-[#8e8e93] w-full">dario | (silence with DARIO_QUIET_TLS=1, or use --strict-tls to hard-fail)</p>
          <p className="relative shrink-0 text-[#0a84ff] w-full">{`dario - http://localhost:50932`}</p>
          <p className="relative shrink-0 text-[#30d158] w-full">Your Claude subscription is now an API.</p>
          <p className="relative shrink-0 text-[#aeaeb2] w-full">Usage:</p>
          <p className="relative shrink-0 text-[#aeaeb2] w-full">{` ANTHROPIC_BASE_URL=http://localhost:50932`}</p>
          <p className="relative shrink-0 text-[#aeaeb2] w-full">{` ANTHROPIC_API_KEY=dario`}</p>
          <p className="relative shrink-0 text-[#30d158] w-full">OAuth: healthy (expires in 3h 28m)</p>
          <p className="relative shrink-0 text-[#e5e5ea] w-full">Model: passthrough (client decides)</p>
          <p className="relative shrink-0 text-[#8e8e93] w-full">{`dario | 1 account (a pool of one) - add more with 'dario accounts add <alias>' to load-balance`}</p>
        </div>
      </div>
      <div aria-hidden className="absolute border border-[rgba(255,255,255,0.07)] border-solid inset-0 pointer-events-none rounded-[8px]" />
    </div>
  );
}

function TerminalLogSection() {
  return (
    <div className="flex-[1_0_0] min-h-px relative w-full" data-name="TerminalLogSection">
      <div className="overflow-clip rounded-[inherit] size-full">
        <div className="content-stretch flex flex-col gap-[8px] items-start pb-[16px] px-[16px] relative size-full">
          <TabBar />
          <ConsoleOutput />
        </div>
      </div>
    </div>
  );
}

function MainDashboard() {
  return (
    <div className="content-stretch flex flex-[1_0_0] flex-col h-full items-start min-w-px relative" data-name="MainDashboard">
      <Header />
      <SubtitleHelp />
      <GenerationSection />
      <PromptCacheSection />
      <TerminalLogSection />
    </div>
  );
}

export default function DarioProxyStatus() {
  return (
    <div className="bg-[#1c1c1e] content-stretch flex items-start relative size-full" data-name="dario-proxy-status">
      <Container />
      <MainDashboard />
    </div>
  );
}