# Stack recipe: C# / .NET with Windows Forms

Status: **planned**. The native .NET runtime (`assets/dotnet/`: `AiEnablement.Core` and `AiEnablement.WinForms`) is
not in the skill yet, so detect.mjs says STOP for this stack. ASP.NET Core and Blazor apps are not this stack: their
UI is HTML, so they use `stacks/web/STACK.md`, "ASP.NET Core / Blazor". WPF, WinUI and MAUI are recognised but have
no recipe; WPF is planned on the same `AiEnablement.Core`.

## What to do today

The same as `stacks/rust-egui/STACK.md`, "What to do today":

- Never copy the web runtime in, and never hand-write a drawer.
- Offer the survey and plan, and the `ai/` folder's skills, agents, toolsets, permissions and memories (`spec/`).
- Treat the API names below as provisional.

## 1. Survey: what to look for in a WinForms app

In addition to the portable survey in SKILL.md:

- **The main form** and how screens change: `UserControl`s swapped into a panel, `TabControl` pages, MDI children,
  separate forms. Each is a page: id, title, and the state behind it (view models, services, data-bound
  `BindingSource`s, not the controls).
- **`host`**: the services object the forms already call (or the DI container's services). Note which actions live
  only in event handlers (`button_Click`). A tool must not click buttons; move the logic into a method both the handler
  and the tool call, as a small refactor in the plan.
- **Threading**: `async` handlers and `BackgroundWorker`. Everything that touches controls runs on the UI thread.
- **Layout**: where a right-docked panel fits (`Dock = Right` with a `Splitter`, or a `SplitContainer`), minimum
  window size, `AutoScaleMode` and DPI.
- **Keyboard**: `KeyPreview`, `ProcessCmdKey` overrides, menu shortcuts that would steal keys from the drawer.
- **Policy constraints**, **existing AI code** and **tests** (xUnit/NUnit/MSTest; any UI automation such as FlaUI) as
  in the portable survey.

## 3. Install the runtime

Copy `assets/dotnet/AiEnablement.Core/` and `assets/dotnet/AiEnablement.WinForms/` unchanged into the solution
(`src/AiEnablement.Core/`, `src/AiEnablement.WinForms/`), add both to the `.sln`, and add `ProjectReference`s from
the app. They are framework-owned and fingerprinted. The Core project targets the app's .NET version (net8.0+);
WinForms targets `net8.0-windows`.

## 4. The capability folder

`ai/` beside the app project: `scaffold.mjs ai init`. Ship it either way:

- copied to the output (`<None Include="ai\**" CopyToOutputDirectory="PreserveNewest" />`), so the runtime reads it
  next to the executable;
- or as `EmbeddedResource`.

The index names a tool catalog (§9).

## 5. Mount the agent once

One app-owned file, e.g. `Ai/AiSetup.cs`:

- `AiAgent.Create(appId, options)` takes app info, the system prompt, the capability source, the tool registry (§9) and
  the hooks:

  | Hook | Default |
  | --- | --- |
  | `ProviderPolicy` | none |
  | `BeforeSend` | none |
  | `ISettingsStore` | `%APPDATA%\<company>\<app>\ai-settings.json` |
  | `IKeyStore` | Windows Credential Manager, or DPAPI `ProtectedData` |
  | `IMemoryStore`, `IChatStore` | files; `null` turns persistence off |
- Add an `AiAgentPanel` (a `UserControl`) to the main form, docked right with a splitter.
- An "Ask AI" button or menu item and Ctrl+I toggle it.
- Tool handlers run on the UI thread (the panel's `SynchronizationContext`); model streaming runs on the thread pool.

## 6. Pages and 7. Change signal

- **Pages**: `agent.SetPage(new PageContext { Id, Title, Purpose, Content = () => …, View = () => … })` when the
  screen changes.
- **Content builders** are pure static methods (state → string) with xUnit tests.
- **Change signal**: call `agent.ContextChanged()` from the services' change events, `BindingSource.ListChanged`, and
  after saves.

## 8. Fit the host app

- The panel takes the app's colours and fonts (`Font`, `ForeColor`, a theme object).
- It isolates its keys (`ProcessCmdKey` returns early while the panel has focus).
- Its confirmations are cards inside the panel, not `MessageBox`es.

## 9. Tools

- **Shape**: toolset classes in `Ai/Tools/`, each returning tools built with
  `AiTool.Create("name").Title(…).Effect(Effect.Write).Param(…).Run((args, ctx) => ctx.Host.Orders.SetFilter(…))`.
  Or use a class per tool implementing `IAiTool`.
- **Handlers** call the services, never the controls.
- **Catalog test**: an xUnit test exports `ai/tools.catalog.json` and fails when it is stale
  (`AI_CATALOG_UPDATE=1` rewrites it). See `spec/tool-catalog.md`.

## 11. Memory, vision, attachments

- **Memory**: files or the app's store, or off.
- **Screenshots**: `Control.DrawToBitmap` of the main form's content (the panel excluded), scaled to 1280 px JPEG.
- **Attachments**: `OpenFileDialog` and drag and drop. Text extraction for plain text, plus the app's own readers.

## 13. Transport

Direct with `HttpClient` (SSE read as `IAsyncEnumerable`). There is no relay, and keys live in the OS keychain.
`ProviderPolicy.LoopbackOnly` applies where the app allows only local models.

## 14. Verify

- `validate.mjs`, then `dotnet test`: content builders, toolsets, catalog, and the conformance vectors in the runtime's
  own tests.
- **The flow**: drive the panel's view model against a mock model server: ask → *synced* → change → *dirty* → tool
  confirmation → result.
- **By hand**, maximised at 1920×1080: the flow against the real model, and every Settings tab.

## 15. Manifest

`"stack": { "id": "dotnet-winforms", "ui": "WinForms net8.0-windows" }`. `framework.runtime` names the two project
folders.
