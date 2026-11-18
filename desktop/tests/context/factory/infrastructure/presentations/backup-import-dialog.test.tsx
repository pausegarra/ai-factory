import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { BackupImportDialog } from "../../../../../src/context/factory/infrastructure/presentations/backup-import-dialog";
afterEach(cleanup);
const prepared={confirmation_id:"ticket",replace:["history"] as const,preserve:["configuration","credentials"] as const,counts:{runs:2},warnings:[]};
it("focuses cancel and supports Escape without applying",()=>{const cancel=vi.fn();const confirm=vi.fn();render(<BackupImportDialog prepared={{...prepared,replace:[...prepared.replace],preserve:[...prepared.preserve]}} busy={false} onCancel={cancel} onConfirm={confirm}/>);expect(document.activeElement).toBe(screen.getByRole("button",{name:"Cancel"}));fireEvent.keyDown(screen.getByRole("alertdialog"),{key:"Escape"});expect(cancel).toHaveBeenCalledTimes(1);expect(confirm).not.toHaveBeenCalled();});
it("ignores Escape and disables cancellation while applying",()=>{const cancel=vi.fn();render(<BackupImportDialog prepared={{...prepared,replace:[...prepared.replace],preserve:[...prepared.preserve]}} busy onCancel={cancel} onConfirm={vi.fn()}/>);fireEvent.keyDown(screen.getByRole("alertdialog"),{key:"Escape"});expect(cancel).not.toHaveBeenCalled();expect((screen.getByRole("button",{name:"Cancel"}) as HTMLButtonElement).disabled).toBe(true);});
