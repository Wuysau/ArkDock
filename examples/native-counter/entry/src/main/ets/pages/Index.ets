@Entry
@Component
struct Index {
  @State count: number = 0;

  aboutToAppear(): void {
    console.info('ARKDOCK_NATIVE_REVISION=FIRST');
  }

  build() {
    Column({ space: 24 }) {
      Text('ArkDock Native Counter')
        .fontSize(26)
        .fontWeight(FontWeight.Bold)
        .fontColor('#16345c')
        .id('native-title')
      Text(`Count: ${this.count}`)
        .fontSize(42)
        .fontColor('#1479ff')
        .id('native-count')
      Button('Increment actual ArkUI state')
        .fontSize(16)
        .height(56)
        .width('85%')
        .id('native-increment')
        .onClick(() => {
          this.count += 1;
          console.info(`ARKDOCK_NATIVE_COUNT=${this.count}`);
        })
      Text('Source revision: FIRST')
        .fontSize(18)
        .fontColor('#52647b')
        .id('native-revision')
    }
    .width('100%')
    .height('100%')
    .justifyContent(FlexAlign.Center)
    .backgroundColor('#eef5ff')
  }
}
