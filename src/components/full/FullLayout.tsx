import { useApp } from "../../app/view";
import { HistoryNav } from "../common/HistoryNav";
import { RecordButton } from "../common/RecordButton";
import { LevelMeter, SilenceNote } from "../common/RecordingInfo";
import { StyleChips } from "../common/StyleChips";
import { AnswerPanel } from "./AnswerPanel";
import { AskBox, NoticeBar, ProfileControls, QuestionPanel } from "./Controls";
import { Header } from "./Header";
import { StatusLine } from "./StatusLine";

/** Full layout: header, status, ANSWER FIRST (camera level), then controls. */
export function FullLayout() {
  const { view } = useApp();
  const coreReady = view.core === "ready";
  return (
    <div className="layout layout--full" data-testid="full-layout">
      <Header />
      {view.core === "starting" && (
        <p className="banner banner--info" data-testid="core-starting">
          Starting…
        </p>
      )}
      {view.core === "failed" && (
        <div className="alert alert--error" role="alert" data-testid="core-failed">
          <p className="alert__text">{view.coreError?.message ?? "The app core failed to start."}</p>
        </div>
      )}
      <StatusLine />
      <NoticeBar />
      <main className="layout__main">
        <AnswerPanel />
        <QuestionPanel />
        <fieldset className="plain-fieldset controls" disabled={!coreReady}>
          <legend className="visually-hidden">Recording controls</legend>
          <div className="record-row">
            <RecordButton />
            <LevelMeter />
          </div>
          <SilenceNote />
          <p className="hotkey-hint" data-testid="hotkey-hint">
            {view.hotkeyHint}
          </p>
          <AskBox />
          <ProfileControls />
          <StyleChips />
          <HistoryNav />
        </fieldset>
      </main>
    </div>
  );
}
