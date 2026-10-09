@Entry
@Component
struct Index {
  @State count: number = 0
  @State message: string = ''

  build() {
    Column({ space: 16 }) {
      Text(`ArkDock native fixture: ${this.count}`)
      Button('Increment').onClick(() => { this.count += 1 })
      TextInput({ placeholder: 'Type in the native runtime', text: this.message })
        .onChange((value: string) => { this.message = value })
      Text(this.message)
    }.width('100%').height('100%').justifyContent(FlexAlign.Center)
  }
}
