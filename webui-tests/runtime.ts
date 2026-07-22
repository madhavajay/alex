export type TestRuntime = {
  baseUrl: string;
  localKey: string;
  fakeBaseUrl: string;
  fakeControlKey: string;
  daemonPid: number;
  fakeprovPid: number;
  tempDir: string;
};
