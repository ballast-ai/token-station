"use client"

import * as React from "react"
import { motion } from "motion/react"
import { Select as SelectPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"
import { ChevronDownIcon, CheckIcon, ChevronUpIcon } from "lucide-react"
import {
  DROPDOWN_CHEVRON_TRANSITION,
  REDUCED_SELECT_SURFACE_VARIANTS,
  SELECT_SURFACE_VARIANTS,
  SELECT_SURFACE_TRANSITION,
  useDropdownReducedMotion,
} from "./dropdown-motion"

interface SelectMotionContextValue {
  open: boolean
  reduceMotion: boolean
  setOpen: (open: boolean) => void
}

const SelectMotionContext = React.createContext<SelectMotionContextValue | null>(null)

function useSelectMotionContext(component: string) {
  const context = React.useContext(SelectMotionContext)
  if (!context) throw new Error(`${component} must be used within <Select>`)
  return context
}

function Select({
  open: controlledOpen,
  defaultOpen = false,
  onOpenChange,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Root>) {
  const reduceMotion = useDropdownReducedMotion()
  const [internalOpen, setInternalOpen] = React.useState(defaultOpen)
  const open = controlledOpen ?? internalOpen
  const handleOpenChange = React.useCallback((nextOpen: boolean) => {
    if (controlledOpen === undefined) setInternalOpen(nextOpen)
    onOpenChange?.(nextOpen)
  }, [controlledOpen, onOpenChange])
  const context = React.useMemo(
    () => ({ open, reduceMotion, setOpen: handleOpenChange }),
    [handleOpenChange, open, reduceMotion],
  )

  return (
    <SelectMotionContext.Provider value={context}>
      <SelectPrimitive.Root
        data-slot="select"
        {...props}
        open={open}
        onOpenChange={handleOpenChange}
      />
    </SelectMotionContext.Provider>
  )
}

function SelectGroup({
  className,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Group>) {
  return (
    <SelectPrimitive.Group
      data-slot="select-group"
      className={cn("scroll-my-1 p-1", className)}
      {...props}
    />
  )
}

function SelectValue({
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Value>) {
  return <SelectPrimitive.Value data-slot="select-value" {...props} />
}

function SelectTrigger({
  className,
  size = "default",
  children,
  onClick,
  onPointerDown,
  style,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Trigger> & {
  size?: "sm" | "default"
}) {
  const { open, reduceMotion, setOpen } = useSelectMotionContext("SelectTrigger")
  const closeFromPointerRef = React.useRef(false)
  const pointerStartedOpenRef = React.useRef<boolean | null>(null)

  return (
    <SelectPrimitive.Trigger
      data-slot="select-trigger"
      data-size={size}
      className={cn(
        "flex w-fit items-center justify-between gap-1.5 rounded-lg border border-input bg-transparent py-2 pr-2 pl-2.5 text-sm whitespace-nowrap transition-[color,background-color,border-color,box-shadow,border-radius] outline-none select-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 data-placeholder:text-muted-foreground data-[size=default]:h-8 data-[size=sm]:h-7 data-[size=sm]:rounded-[min(var(--radius-md),10px)] data-[state=open]:border-foreground/70 *:data-[slot=select-value]:line-clamp-1 *:data-[slot=select-value]:flex *:data-[slot=select-value]:items-center *:data-[slot=select-value]:gap-1.5 dark:bg-input/30 dark:hover:bg-input/50 dark:aria-invalid:border-destructive/50 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
        className
      )}
      style={{ ...style, pointerEvents: open ? "auto" : style?.pointerEvents }}
      onPointerDown={(event) => {
        onPointerDown?.(event)
        pointerStartedOpenRef.current = open
        if (!event.defaultPrevented && open) {
          closeFromPointerRef.current = true
          event.preventDefault()
          setOpen(false)
        }
      }}
      onClick={(event) => {
        onClick?.(event)
        if (event.defaultPrevented) return
        if (closeFromPointerRef.current) {
          closeFromPointerRef.current = false
          pointerStartedOpenRef.current = null
          event.preventDefault()
          return
        }
        const pointerStartedOpen = pointerStartedOpenRef.current
        pointerStartedOpenRef.current = null
        if (pointerStartedOpen !== false && open) {
          event.preventDefault()
          setOpen(false)
        }
      }}
      {...props}
    >
      {children}
      <SelectPrimitive.Icon asChild>
        <motion.span
          className="pointer-events-none inline-flex size-4 text-muted-foreground"
          animate={{ rotate: open ? 180 : 0 }}
          transition={reduceMotion ? { duration: 0 } : DROPDOWN_CHEVRON_TRANSITION}
        >
          <ChevronDownIcon className="size-4" data-motion-dropdown-chevron="true" />
        </motion.span>
      </SelectPrimitive.Icon>
    </SelectPrimitive.Trigger>
  )
}

function SelectContent({
  className,
  children,
  position = "popper",
  align = "center",
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Content>) {
  const { open, reduceMotion } = useSelectMotionContext("SelectContent")

  return (
    <SelectPrimitive.Portal>
      <SelectPrimitive.Content
        data-slot="select-content"
        data-motion-dropdown="select"
        data-align-trigger={position === "item-aligned"}
        className={cn(
          "relative z-50 max-h-(--radix-select-content-available-height) min-w-36 overflow-visible outline-none",
          position === "popper" && "data-[side=bottom]:translate-y-1 data-[side=left]:-translate-x-1 data-[side=right]:translate-x-1 data-[side=top]:-translate-y-1",
        )}
        position={position}
        align={align}
        {...props}
      >
        <motion.div
          aria-hidden={open ? undefined : true}
          data-motion-dropdown-surface="true"
          initial={reduceMotion ? false : "closed"}
          animate={open ? "open" : "closed"}
          variants={reduceMotion ? REDUCED_SELECT_SURFACE_VARIANTS : SELECT_SURFACE_VARIANTS}
          transition={reduceMotion ? { duration: 0 } : SELECT_SURFACE_TRANSITION}
          className={cn(
            "max-h-(--radix-select-content-available-height) min-w-full overflow-x-hidden overflow-y-auto rounded-lg bg-popover text-popover-foreground shadow-md ring-1 ring-foreground/10",
            className,
          )}
          style={{ transformOrigin: "var(--radix-select-content-transform-origin)" }}
        >
          <SelectScrollUpButton />
          <SelectPrimitive.Viewport
            data-position={position}
            data-motion-dropdown-list="true"
            className="data-[position=popper]:min-h-(--radix-select-trigger-height) data-[position=popper]:w-full data-[position=popper]:min-w-(--radix-select-trigger-width)"
          >
            {children}
          </SelectPrimitive.Viewport>
          <SelectScrollDownButton />
        </motion.div>
      </SelectPrimitive.Content>
    </SelectPrimitive.Portal>
  )
}

function SelectLabel({
  className,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Label>) {
  return (
    <SelectPrimitive.Label
      data-slot="select-label"
      className={cn("px-1.5 py-1 text-xs text-muted-foreground", className)}
      {...props}
    />
  )
}

function SelectItem({
  className,
  children,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Item>) {
  return (
    <SelectPrimitive.Item
      data-slot="select-item"
      className={cn(
        "motion-dropdown-item relative flex w-full cursor-default items-center gap-1.5 rounded-md py-1 pr-8 pl-1.5 text-sm outline-hidden select-none focus:bg-accent focus:text-accent-foreground not-data-[variant=destructive]:focus:**:text-accent-foreground data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4 *:[span]:last:flex *:[span]:last:items-center *:[span]:last:gap-2",
        className
      )}
      {...props}
    >
      <span className="pointer-events-none absolute right-2 flex size-4 items-center justify-center">
        <SelectPrimitive.ItemIndicator>
          <CheckIcon className="pointer-events-none" />
        </SelectPrimitive.ItemIndicator>
      </span>
      <SelectPrimitive.ItemText>{children}</SelectPrimitive.ItemText>
    </SelectPrimitive.Item>
  )
}

function SelectSeparator({
  className,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Separator>) {
  return (
    <SelectPrimitive.Separator
      data-slot="select-separator"
      className={cn("pointer-events-none -mx-1 my-1 h-px bg-border", className)}
      {...props}
    />
  )
}

function SelectScrollUpButton({
  className,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.ScrollUpButton>) {
  return (
    <SelectPrimitive.ScrollUpButton
      data-slot="select-scroll-up-button"
      className={cn(
        "z-10 flex cursor-default items-center justify-center bg-popover py-1 [&_svg:not([class*='size-'])]:size-4",
        className
      )}
      {...props}
    >
      <ChevronUpIcon />
    </SelectPrimitive.ScrollUpButton>
  )
}

function SelectScrollDownButton({
  className,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.ScrollDownButton>) {
  return (
    <SelectPrimitive.ScrollDownButton
      data-slot="select-scroll-down-button"
      className={cn(
        "z-10 flex cursor-default items-center justify-center bg-popover py-1 [&_svg:not([class*='size-'])]:size-4",
        className
      )}
      {...props}
    >
      <ChevronDownIcon />
    </SelectPrimitive.ScrollDownButton>
  )
}

export {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectScrollDownButton,
  SelectScrollUpButton,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
}
