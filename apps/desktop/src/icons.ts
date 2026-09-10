// Central Font Awesome registration so the bundle only ships the icons we use.
import { library } from "@fortawesome/fontawesome-svg-core";
import {
  faChevronDown, faMagnifyingGlassChart, faEllipsis, faThumbtack, faPen, faBoxArchive,
  faChevronRight, faChevronUp, faChevronLeft, faCodeBranch, faArrowUpRightFromSquare, faCircleInfo, faGlobe, faXmark,
  faPlus, faTerminal, faCheck, faArrowUp, faArrowDown, faArrowLeft, faArrowRight, faRotateRight, faRotateLeft,
  faExpand, faMagnifyingGlass, faMagnifyingGlassPlus, faCaretUp, faCaretDown, faCaretLeft, faCaretRight, faCode, faGear, faLayerGroup,
  faPuzzlePiece, faCircleUser, faArrowRightFromBracket, faCube, faFile, faFileCsv, faFileWord, faArrowPointer,
  faFileLines as faFileLinesSolid, faEnvelope, faFileExcel, faRobot, faCloud, faCloudArrowDown, faBullseye,
  faBorderAll, faMinus, faSliders, faServer, faAnchor, faLink, faLeaf, faDesktop,
  faCircleCheck as faCircleCheckSolid, faLaptop, faCompress, faFolderPlus, faFileExport, faClockRotateLeft,
  faKey, faLock, faMoon, faCircleNotch, faHand, faCircleExclamation, faWrench, faListCheck, faShieldHalved,
  faBook, faToggleOn, faToggleOff, faTriangleExclamation, faNoteSticky, faTrash, faShareNodes, faList, faInbox, faLightbulb, faStop, faBolt, faCoins, faWallet,
  faStar, faPlay, faFolderTree, faMicrophone, faPause, faDownload, faTable, faTableColumns, faAnglesLeft, faAnglesRight, faUpload, faPaperclip,
  faFolder as faFolderSolid, faRotate, faFolderOpen as faFolderOpenSolid, faFileZipper,
  faSpinner, faCodeCommit, faPlug, faWandMagicSparkles, faTags,
  faImage as faImageSolid, faLeftRight, faVideo, faFilm, faFileCode, faUserCheck,
  faSquare as faSquareSolid, faClock as faClockSolid,
  // FA5 legacy aliases (deprecated names still used in some components)
  faXmark as faTimes, faRotate as faSyncAlt
} from "@fortawesome/free-solid-svg-icons";

import {
  faClock, faBell, faCalendarCheck, faWindowRestore, faCopy, faFileLines as faFileLinesRegular,
  faFolderOpen, faCommentDots, faComments, faPenToSquare, faFolder, faMessage, faSquare,
  faCircleCheck as faCircleCheckRegular, faSun, faFaceSmile, faKeyboard, faCompass, faTrashCan, faWindowMaximize,
  faImage, faFilePdf, faFileWord as faFileWordRegular, faFileExcel as faFileExcelRegular, faFilePowerpoint as faFilePowerpointRegular,
  faFile as faFileRegular
} from "@fortawesome/free-regular-svg-icons";

import { faChrome, faFigma, faGitAlt, faGithub, faReact } from "@fortawesome/free-brands-svg-icons";

const solidIcons = [
  faChevronDown, faMagnifyingGlassChart, faEllipsis, faThumbtack, faPen, faBoxArchive,
  faChevronRight, faChevronUp, faChevronLeft, faCodeBranch, faArrowUpRightFromSquare, faCircleInfo, faGlobe, faXmark,
  faPlus, faTerminal, faCheck, faArrowUp, faArrowDown, faArrowLeft, faArrowRight, faRotateRight, faRotateLeft,
  faExpand, faMagnifyingGlass, faMagnifyingGlassPlus, faCaretUp, faCaretDown, faCaretLeft, faCaretRight, faCode, faGear, faLayerGroup,
  faPuzzlePiece, faCircleUser, faArrowRightFromBracket, faCube, faFile, faFileCsv, faFileWord, faArrowPointer,
  faFileLinesSolid, faEnvelope, faFileExcel, faRobot, faCloud, faCloudArrowDown, faBullseye, faBorderAll, faMinus,
  faSliders, faServer, faAnchor, faLink, faLeaf, faDesktop, faCircleCheckSolid, faLaptop,
  faCompress, faFolderPlus, faFileExport, faClockRotateLeft, faKey, faLock, faMoon, faCircleNotch, faHand, faCircleExclamation, faWrench, faListCheck, faShieldHalved,
  faImageSolid, faLeftRight, faVideo, faFilm, faFileCode, faUserCheck,
  faBook, faToggleOn, faToggleOff, faTriangleExclamation, faNoteSticky, faTrash, faShareNodes, faList, faInbox, faLightbulb, faStop, faBolt, faCoins, faWallet,
  faStar, faPlay, faFolderTree, faMicrophone, faPause, faDownload, faTable, faTableColumns, faAnglesLeft, faAnglesRight, faUpload, faPaperclip,
  faFolderSolid, faRotate, faFolderOpenSolid, faFileZipper,
  faSpinner, faCodeCommit, faPlug, faWandMagicSparkles, faTags,
  faSquareSolid, faClockSolid,
  faTimes, faSyncAlt
];

const regularIcons = [
  faClock, faBell, faCalendarCheck, faWindowRestore, faCopy, faFileLinesRegular,
  faFolderOpen, faCommentDots, faComments, faPenToSquare, faFolder, faMessage, faSquare,
  faCircleCheckRegular, faSun, faFaceSmile, faKeyboard, faCompass, faTrashCan, faWindowMaximize,
  faImage, faFilePdf, faFileWordRegular, faFileExcelRegular, faFilePowerpointRegular,
  faFileRegular
];

const brandIcons = [
  faChrome, faFigma, faGitAlt, faGithub, faReact
];

solidIcons.forEach(icon => library.add(icon));
regularIcons.forEach(icon => library.add(icon));
brandIcons.forEach(icon => library.add(icon));
